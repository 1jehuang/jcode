//! Cursor provider runtime (direct ChatService HTTP/2 streaming), moved out
//! of `jcode-base` so provider edits compile only this crate plus a binary
//! relink instead of rebuilding the base -> app-core -> tui spine. The
//! binary's composition root registers [`CursorCliProvider`] with
//! `jcode_base::provider::external` at startup.
//!
//! The pure model-catalog data (`AVAILABLE_MODELS`, `is_known_model`) stays in
//! `jcode_base::provider::cursor` because base's model-routing logic needs it
//! without a runtime.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use jcode_base::auth::cursor as cursor_auth;
use jcode_base::provider::cursor::{AVAILABLE_MODELS, DEFAULT_MODEL};
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, NativeToolResult, NativeToolResultSender, Provider};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

mod agent_transport;
pub mod wire;

const MODELS_API_URL: &str = "https://api.cursor.com/v0/models";
const MAX_AGENT_MODELS_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PROMPT_CHARS: usize = 120_000;

fn build_cli_prompt(system: &str, messages: &[Message]) -> String {
    let mut out = String::new();

    if !system.trim().is_empty() {
        out.push_str("System:\n");
        out.push_str(system.trim());
        out.push_str("\n\n");
    }

    out.push_str("Conversation:\n");

    for message in messages {
        let role = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        out.push_str(role);
        out.push_str(":\n");

        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::Reasoning { .. }
                | ContentBlock::ReasoningTrace { .. }
                | ContentBlock::AnthropicThinking { .. }
                | ContentBlock::OpenAIReasoning { .. } => {}
                ContentBlock::ToolUse { name, input, .. } => {
                    out.push_str("[tool_use ");
                    out.push_str(name);
                    out.push_str(" input=");
                    out.push_str(&input.to_string());
                    out.push_str("]\n");
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    out.push_str("[tool_result ");
                    out.push_str(tool_use_id);
                    out.push_str(" is_error=");
                    out.push_str(if is_error.unwrap_or(false) {
                        "true"
                    } else {
                        "false"
                    });
                    out.push_str("]\n");
                    out.push_str(content);
                    out.push('\n');
                }
                ContentBlock::Image { .. } => {
                    out.push_str("[image]\n");
                }
                ContentBlock::OpenAICompaction { .. } => {
                    out.push_str("[openai native compaction]\n");
                }
            }
        }
        out.push('\n');
    }

    out.push_str("Assistant:\n");

    if out.chars().count() <= MAX_PROMPT_CHARS {
        return out;
    }

    let mut kept = out.chars().rev().take(MAX_PROMPT_CHARS).collect::<Vec<_>>();
    kept.reverse();
    let tail: String = kept.into_iter().collect();
    format!(
        "[Earlier conversation truncated to fit prompt limits]\n\n{}",
        tail
    )
}

#[derive(Debug, Deserialize)]
struct CursorModelsResponse {
    #[serde(default)]
    models: Vec<String>,
}

/// Decode the model ids returned by Cursor's native
/// `agent.v1.AgentService/GetUsableModels` endpoint.
///
/// Cursor's CLI uses a raw protobuf unary response rather than the Connect
/// streaming envelope used by `Run`. The response contains repeated
/// `ModelDetails` messages in field 1, and `ModelDetails.model_id` is field 1.
/// Keep this parser deliberately small and forward-compatible: unknown fields
/// are skipped, while malformed/truncated input is rejected.
fn decode_agent_models(mut payload: &[u8]) -> Result<Vec<String>> {
    if payload.len() >= 5 && (payload[0] == 0 || payload[0] == 1) {
        let framed_len =
            u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
        if framed_len == payload.len().saturating_sub(5) {
            payload = &payload[5..];
        }
    }

    let mut models = Vec::new();
    for field in protobuf_fields(payload)? {
        if field.number != 1 || field.wire_type != 2 {
            continue;
        }
        let model_id = protobuf_fields(field.data)?
            .into_iter()
            .find(|nested| nested.number == 1 && nested.wire_type == 2)
            .and_then(|nested| std::str::from_utf8(nested.data).ok())
            .map(str::trim)
            .filter(|model| !model.is_empty());
        if let Some(model) = model_id
            && !models.iter().any(|known| known == model)
        {
            models.push(model.to_string());
        }
    }
    Ok(models)
}

#[derive(Debug)]
struct ProtobufField<'a> {
    number: u64,
    wire_type: u8,
    data: &'a [u8],
}

fn protobuf_fields(mut payload: &[u8]) -> Result<Vec<ProtobufField<'_>>> {
    let mut fields = Vec::new();
    while !payload.is_empty() {
        let (tag, rest) = read_protobuf_varint(payload)?;
        payload = rest;
        let number = tag >> 3;
        let wire_type = (tag & 7) as u8;
        if number == 0 {
            anyhow::bail!("Cursor model catalog contained an invalid protobuf field number");
        }
        match wire_type {
            0 => {
                let (_, rest) = read_protobuf_varint(payload)?;
                payload = rest;
                fields.push(ProtobufField {
                    number,
                    wire_type,
                    data: &[],
                });
            }
            1 => {
                if payload.len() < 8 {
                    anyhow::bail!("Cursor model catalog protobuf was truncated");
                }
                payload = &payload[8..];
                fields.push(ProtobufField {
                    number,
                    wire_type,
                    data: &[],
                });
            }
            2 => {
                let (length, rest) = read_protobuf_varint(payload)?;
                let length = usize::try_from(length)
                    .context("Cursor model catalog protobuf length overflowed")?;
                if rest.len() < length {
                    anyhow::bail!("Cursor model catalog protobuf was truncated");
                }
                fields.push(ProtobufField {
                    number,
                    wire_type,
                    data: &rest[..length],
                });
                payload = &rest[length..];
            }
            5 => {
                if payload.len() < 4 {
                    anyhow::bail!("Cursor model catalog protobuf was truncated");
                }
                payload = &payload[4..];
                fields.push(ProtobufField {
                    number,
                    wire_type,
                    data: &[],
                });
            }
            _ => anyhow::bail!("Cursor model catalog used unsupported protobuf wire type"),
        }
    }
    Ok(fields)
}

fn read_protobuf_varint(payload: &[u8]) -> Result<(u64, &[u8])> {
    let mut value = 0u64;
    for (index, byte) in payload.iter().copied().enumerate() {
        if index >= 10 {
            anyhow::bail!("Cursor model catalog protobuf varint overflowed");
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok((value, &payload[index + 1..]));
        }
    }
    anyhow::bail!("Cursor model catalog protobuf varint was truncated")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedCatalog {
    models: Vec<String>,
    fetched_at_rfc3339: String,
}

fn merge_cursor_models(dynamic: &[String], current: &str) -> Vec<String> {
    let mut merged = Vec::new();

    for model in dynamic {
        let trimmed = model.trim();
        if !trimmed.is_empty() && !merged.iter().any(|known| known == trimmed) {
            merged.push(trimmed.to_string());
        }
    }

    for model in AVAILABLE_MODELS {
        let trimmed = model.trim();
        if !trimmed.is_empty() && !merged.iter().any(|known| known == trimmed) {
            merged.push(trimmed.to_string());
        }
    }

    let current = current.trim();
    if !current.is_empty() && !merged.iter().any(|known| known == current) {
        merged.push(current.to_string());
    }

    merged
}

enum CursorModelsAuth<'a> {
    ApiKey(&'a str),
    Bearer(&'a str),
}

async fn fetch_available_models(
    client: &reqwest::Client,
    auth: CursorModelsAuth<'_>,
) -> Result<Vec<String>> {
    let request = client.get(MODELS_API_URL);
    let request = match auth {
        CursorModelsAuth::ApiKey(api_key) => request.basic_auth(api_key, Some("")),
        CursorModelsAuth::Bearer(access_token) => request.bearer_auth(access_token),
    };
    let response = request
        .send()
        .await
        .context("Failed to fetch Cursor model catalog")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "Cursor model catalog request failed ({}): {}",
            status,
            body.trim()
        );
    }

    let parsed: CursorModelsResponse = response
        .json()
        .await
        .context("Failed to decode Cursor model catalog response")?;
    Ok(parsed
        .models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect())
}

async fn fetch_agent_models(client: &reqwest::Client, access_token: &str) -> Result<Vec<String>> {
    let host = agent_transport::agent_host();
    let url = format!("https://{host}/agent.v1.AgentService/GetUsableModels");
    let response = client
        .post(url)
        .header("authorization", format!("Bearer {access_token}"))
        .header("content-type", "application/proto")
        .header("connect-protocol-version", "1")
        .header("x-ghost-mode", "true")
        .header("x-cursor-client-type", "cli")
        .header(
            "x-cursor-client-version",
            agent_transport::cli_client_version(),
        )
        .header(
            "x-session-id",
            cursor_auth::session_id_for_access_token(access_token),
        )
        .body(Vec::<u8>::new())
        .send()
        .await
        .context("Failed to fetch Cursor AgentService model catalog")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "Cursor AgentService model catalog request failed ({}): {}",
            status,
            body.trim()
        );
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AGENT_MODELS_RESPONSE_BYTES)
    {
        anyhow::bail!("Cursor AgentService model catalog response exceeded 4 MiB");
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Failed to read Cursor AgentService model catalog")?
    {
        if body.len() as u64 + chunk.len() as u64 > MAX_AGENT_MODELS_RESPONSE_BYTES {
            anyhow::bail!("Cursor AgentService model catalog response exceeded 4 MiB");
        }
        body.extend_from_slice(&chunk);
    }
    decode_agent_models(&body)
}

fn runtime_cursor_api_key() -> Option<String> {
    jcode_base::auth::cursor::load_api_key().ok()
}

#[derive(Clone, Default)]
pub struct ToolResultMultiplexer {
    routes: Arc<Mutex<HashMap<String, mpsc::Sender<NativeToolResult>>>>,
}

impl ToolResultMultiplexer {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(
        &self,
        stream_uuid: &str,
        tx: mpsc::Sender<NativeToolResult>,
    ) -> StreamGuard {
        if let Ok(mut routes) = self.routes.lock() {
            routes.insert(stream_uuid.to_string(), tx);
        }
        StreamGuard {
            stream_uuid: stream_uuid.to_string(),
            routes: self.routes.clone(),
        }
    }

    pub fn dispatch(&self, result: NativeToolResult) {
        let stream_uuid = result
            .request_id
            .split(':')
            .next()
            .unwrap_or(&result.request_id);
        let sender = self
            .routes
            .lock()
            .ok()
            .and_then(|routes| routes.get(stream_uuid).cloned());
        if let Some(sender) = sender {
            tokio::spawn(async move {
                let _ = sender.send(result).await;
            });
        }
    }
}

pub struct StreamGuard {
    stream_uuid: String,
    routes: Arc<Mutex<HashMap<String, mpsc::Sender<NativeToolResult>>>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.remove(&self.stream_uuid);
        }
    }
}

pub struct CursorCliProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
    multiplexer: ToolResultMultiplexer,
    result_tx: NativeToolResultSender,
    loop_started: Arc<std::sync::atomic::AtomicBool>,
    result_rx: Arc<Mutex<Option<mpsc::Receiver<NativeToolResult>>>>,
}

impl CursorCliProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(jcode_base::storage::app_config_dir()?.join("cursor_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        jcode_base::storage::read_json(&path)
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
        if let Err(error) = jcode_base::storage::write_json(&path, &payload) {
            jcode_base::logging::warn(&format!(
                "Failed to persist Cursor model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog()
            && let Ok(mut models) = self.fetched_models.write()
        {
            *models = catalog.models;
        }
    }

    fn ensure_forwarder_loop(&self) {
        if !self
            .loop_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
            && let Ok(mut rx_opt) = self.result_rx.lock()
            && let Some(mut rx) = rx_opt.take()
        {
            let multiplexer = self.multiplexer.clone();
            tokio::spawn(async move {
                while let Some(result) = rx.recv().await {
                    multiplexer.dispatch(result);
                }
            });
        }
    }

    pub fn new() -> Self {
        let model = std::env::var("JCODE_CURSOR_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let multiplexer = ToolResultMultiplexer::new();
        let (result_tx, result_rx) = mpsc::channel::<NativeToolResult>(64);
        let provider = Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
            multiplexer,
            result_tx,
            loop_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            result_rx: Arc::new(Mutex::new(Some(result_rx))),
        };
        provider.seed_cached_catalog();
        provider
    }
}

impl Default for CursorCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CursorCliProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_cli_prompt(system, messages);
        let model = self
            .model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let prompt_items = vec![Value::String(prompt.clone())];
        let system_value = (!system.trim().is_empty()).then(|| Value::String(system.to_string()));
        let payload = json!({
            "model": &model,
            "system": system_value.as_ref(),
            "prompt": &prompt,
        });
        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "cursor",
            &model,
            "cursor_cli_prompt",
            &payload,
            &prompt_items,
            system_value.as_ref(),
            None,
            Some(0),
            &[
                ("logical_message_count", messages.len().to_string()),
                ("advertised_tool_count", tools.len().to_string()),
            ],
        );
        let client = self.client.clone();
        let resume_session_id = resume_session_id.map(str::to_string);
        let (tx, rx) = mpsc::channel::<Result<jcode_message_types::StreamEvent>>(100);
        let stream_uuid = uuid::Uuid::new_v4().to_string();
        let (tool_result_tx, tool_result_rx) = mpsc::channel::<NativeToolResult>(16);
        let stream_guard = self.multiplexer.register(&stream_uuid, tool_result_tx);
        let tools = tools.to_vec();
        let system = system.to_string();
        self.ensure_forwarder_loop();

        tokio::spawn(async move {
            let _stream_guard = stream_guard;
            let result = run_native_text_command(
                client,
                tx.clone(),
                &prompt,
                &model,
                None,
                resume_session_id.as_deref(),
                &stream_uuid,
                &tools,
                &system,
                tool_result_rx,
            )
            .await;

            if let Err(err) = result {
                let _ = tx.send(Err(err)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "cursor"
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // See `strip_own_model_prefix`: `--provider cursor` routes through this
        // runtime directly, so session restore hands it `cursor:<model>`.
        let trimmed = jcode_provider_core::strip_own_model_prefix(model, "cursor:");
        if trimmed.is_empty() {
            anyhow::bail!("Cursor model cannot be empty");
        }
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = trimmed.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn available_models_display(&self) -> Vec<String> {
        let dynamic = self
            .fetched_models
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        merge_cursor_models(&dynamic, &self.model())
    }

    fn model_routes(&self) -> Vec<jcode_provider_core::ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| jcode_provider_core::ModelRoute {
                model,
                provider: "Cursor".to_string(),
                api_method: "cursor".to_string(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        // Prefer the API key endpoint for backwards compatibility. When no
        // key is configured, use the same managed/IDE OAuth resolution as the
        // native AgentService transport. This is read-only and failures retain
        // the static and persisted fallback catalog.
        let fetched = if let Some(api_key) = runtime_cursor_api_key() {
            fetch_available_models(&self.client, CursorModelsAuth::ApiKey(&api_key)).await
        } else {
            match cursor_auth::resolve_direct_tokens(&self.client).await {
                Ok(tokens) => match fetch_agent_models(&self.client, &tokens.access_token).await {
                    Ok(models) if !models.is_empty() => Ok(models),
                    Ok(_) => {
                        fetch_available_models(
                            &self.client,
                            CursorModelsAuth::Bearer(&tokens.access_token),
                        )
                        .await
                    }
                    Err(agent_error) => fetch_available_models(
                        &self.client,
                        CursorModelsAuth::Bearer(&tokens.access_token),
                    )
                    .await
                    .with_context(|| {
                        format!("AgentService discovery also failed: {agent_error:#}")
                    }),
                },
                Err(error) => Err(error).context("no Cursor API key or OAuth credentials"),
            }
        };

        match fetched {
            Ok(models) => {
                if !models.is_empty() {
                    jcode_base::logging::info(&format!(
                        "Discovered Cursor models: {}",
                        models.join(", ")
                    ));
                    Self::persist_catalog(&models);
                    *self
                        .fetched_models
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = models;
                }
            }
            Err(err) => {
                jcode_base::logging::warn(&format!(
                    "Cursor model catalog refresh failed; keeping fallback list: {}",
                    err
                ));
            }
        }

        Ok(())
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            fetched_models: self.fetched_models.clone(),
            multiplexer: self.multiplexer.clone(),
            result_tx: self.result_tx.clone(),
            loop_started: self.loop_started.clone(),
            result_rx: self.result_rx.clone(),
        })
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        self.ensure_forwarder_loop();
        Some(self.result_tx.clone())
    }
}

async fn run_native_text_command(
    client: reqwest::Client,
    tx: mpsc::Sender<Result<StreamEvent>>,
    prompt: &str,
    model: &str,
    _account_label: Option<&str>,
    resume_session_id: Option<&str>,
    stream_uuid: &str,
    tools: &[ToolDefinition],
    system: &str,
    mut tool_result_rx: mpsc::Receiver<NativeToolResult>,
) -> Result<()> {
    let tokens = cursor_auth::resolve_direct_tokens(&client).await?;

    // The current Cursor agent transport (`agent.v1.AgentService/Run`) is a
    // paced bidirectional Connect/HTTP2 stream. The old
    // `ChatService/StreamUnifiedChatWithTools` endpoint was decommissioned for
    // API-key / CLI tokens and now returns "Update Required"/payment errors.
    let first_result = crate::agent_transport::run_agent_turn(
        &tokens.access_token,
        prompt,
        model,
        resume_session_id,
        stream_uuid,
        tools,
        system,
        &mut tool_result_rx,
        tx.clone(),
    )
    .await;

    match first_result {
        Ok(()) => Ok(()),
        Err(err) if cursor_auth::error_indicates_not_logged_in(&err) => {
            let refreshed = cursor_auth::refresh_resolved_tokens(&client, &tokens)
                .await
                .with_context(|| {
                    format!("Cursor token was rejected and refresh also failed after: {err:#}")
                })?;
            crate::agent_transport::run_agent_turn(
                &refreshed.access_token,
                prompt,
                model,
                resume_session_id,
                stream_uuid,
                tools,
                system,
                &mut tool_result_rx,
                tx,
            )
            .await
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod cursor_tests;
