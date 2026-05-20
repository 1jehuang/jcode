use super::{EventStream, Provider};
use crate::auth::cursor as cursor_auth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use flate2::read::GzDecoder;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use std::fmt;
use std::io::Read;
use std::sync::{Arc, RwLock};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

const DIRECT_CHAT_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.ChatService/StreamUnifiedChatWithTools";
const MODELS_API_URL: &str = "https://api.cursor.com/v0/models";
const DEFAULT_MODEL: &str = "composer-1.5";
const MAX_PROMPT_CHARS: usize = 120_000;
pub(crate) const AVAILABLE_MODELS: &[&str] = &[
    "composer-2-fast",
    "composer-2",
    "composer-1.5",
    "composer-1",
    "gpt-5.4-high",
    "gpt-5.4-medium",
    "gpt-5.4-low",
    "gpt-5",
    "sonnet-4.6",
    "sonnet-4.6-thinking",
    "opus-4.6",
    "gemini-3.1-pro",
];

pub(crate) fn is_known_model(model: &str) -> bool {
    let trimmed = model.trim();
    AVAILABLE_MODELS.contains(&trimmed)
}

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
                ContentBlock::Reasoning { .. } => {}
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

async fn fetch_available_models(client: &reqwest::Client, api_key: &str) -> Result<Vec<String>> {
    let response = client
        .get(MODELS_API_URL)
        .basic_auth(api_key, Some(""))
        .send()
        .await
        .context("Failed to fetch Cursor model catalog")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = crate::util::http_error_body(response, "HTTP error").await;
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

fn runtime_cursor_api_key() -> Option<String> {
    crate::auth::cursor::load_api_key().ok()
}

pub struct CursorCliProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
}

impl CursorCliProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(crate::storage::app_config_dir()?.join("cursor_models_cache.json"))
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

    pub fn new() -> Self {
        let model = std::env::var("JCODE_CURSOR_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let provider = Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
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
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
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
        super::fingerprint::log_provider_canonical_input(
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
                ("ignored_tool_count", _tools.len().to_string()),
            ],
        );
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel::<Result<crate::message::StreamEvent>>(100);

        tokio::spawn(async move {
            let result = run_native_text_command(client, tx.clone(), &prompt, &model).await;

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
        let trimmed = model.trim();
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

    fn model_routes(&self) -> Vec<super::ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| super::ModelRoute {
                model,
                provider: "Cursor".to_string(),
                api_method: "cursor".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let Some(api_key) = runtime_cursor_api_key() else {
            return Ok(());
        };

        match fetch_available_models(&self.client, &api_key).await {
            Ok(models) => {
                if !models.is_empty() {
                    crate::logging::info(&format!(
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
                crate::logging::warn(&format!(
                    "Cursor model catalog refresh failed; keeping fallback list: {}",
                    err
                ));
            }
        }

        Ok(())
    }

    fn handles_tools_internally(&self) -> bool {
        false
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            fetched_models: self.fetched_models.clone(),
        })
    }
}

async fn run_native_text_command(
    client: reqwest::Client,
    tx: mpsc::Sender<Result<StreamEvent>>,
    prompt: &str,
    model: &str,
) -> Result<()> {
    if tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "native http2".to_string(),
        }))
        .await
        .is_err()
    {
        return Ok(());
    }

    let tokens = cursor_auth::resolve_direct_tokens(&client).await?;
    let body = build_native_request_body(prompt, model);

    let first_result = run_native_text_command_via_curl(
        tx.clone(),
        &tokens.access_token,
        &cursor_auth::session_id_for_access_token(&tokens.access_token),
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        body.clone(),
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
            run_native_text_command_via_curl(
                tx,
                &refreshed.access_token,
                &cursor_auth::session_id_for_access_token(&refreshed.access_token),
                &Uuid::new_v4().to_string(),
                &Uuid::new_v4().to_string(),
                body,
            )
            .await
        }
        Err(err) => Err(err),
    }
}

async fn run_native_text_command_via_curl(
    tx: mpsc::Sender<Result<StreamEvent>>,
    access_token: &str,
    session_id: &str,
    request_id: &str,
    config_version: &str,
    body: Vec<u8>,
) -> Result<()> {
    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "native http2 (curl)".to_string(),
        }))
        .await;

    let checksum = cursor_auth::checksum_for_access_token(access_token);
    let client_key = cursor_auth::client_key_for_access_token(access_token);
    let client_version = cursor_auth::cursor_direct_client_version();

    let body_path = std::env::temp_dir().join(format!("jcode-cursor-{}.bin", Uuid::new_v4()));
    std::fs::write(&body_path, &body).context("Failed writing Cursor request body temp file")?;
    let body_path_str = body_path.to_string_lossy().to_string();

    let mut cmd = Command::new("curl");
    cmd.kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .arg("--http2")
        .arg("--no-progress-meter")
        .arg("-sS")
        .arg("-X")
        .arg("POST")
        .arg(DIRECT_CHAT_URL)
        .arg("-H")
        .arg(format!("authorization: Bearer {access_token}"))
        .arg("-H")
        .arg("accept-encoding: gzip")
        .arg("-H")
        .arg("connect-accept-encoding: gzip")
        .arg("-H")
        .arg("connect-protocol-version: 1")
        .arg("-H")
        .arg("content-type: application/connect+proto")
        .arg("-H")
        .arg("user-agent: connect-es/1.6.1")
        .arg("-H")
        .arg(format!("x-amzn-trace-id: Root={}", Uuid::new_v4()))
        .arg("-H")
        .arg(format!("x-client-key: {client_key}"))
        .arg("-H")
        .arg(format!("x-cursor-checksum: {checksum}"))
        .arg("-H")
        .arg(format!("x-cursor-client-version: {client_version}"))
        .arg("-H")
        .arg(format!("x-cursor-config-version: {config_version}"))
        .arg("-H")
        .arg("x-cursor-client-type: ide")
        .arg("-H")
        .arg(format!("x-cursor-client-os: {}", std::env::consts::OS))
        .arg("-H")
        .arg(format!("x-cursor-client-arch: {}", std::env::consts::ARCH))
        .arg("-H")
        .arg("x-cursor-client-device-type: desktop")
        .arg("-H")
        .arg("x-cursor-timezone: UTC")
        .arg("-H")
        .arg("x-ghost-mode: true")
        .arg("-H")
        .arg(format!("x-request-id: {request_id}"))
        .arg("-H")
        .arg(format!("x-session-id: {session_id}"))
        .arg("--data-binary")
        .arg(format!("@{body_path_str}"));

    let mut child = cmd
        .spawn()
        .context("Failed to spawn curl for Cursor native API")?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture curl stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture curl stderr"))?;

    let stderr_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        let _ = stderr.read_to_end(&mut collected).await;
        String::from_utf8_lossy(&collected).to_string()
    });

    let _ = tx
        .send(Ok(StreamEvent::SessionId(session_id.to_string())))
        .await;
    let mut router = ThinkRouter::default();
    let mut pending = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = stdout
            .read(&mut buf)
            .await
            .context("Failed to read curl Cursor response stream")?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..read]);
        drain_native_frames(&tx, &mut pending, &mut router).await?;
    }

    let status = child
        .wait()
        .await
        .context("Failed waiting for curl process")?;
    let _ = std::fs::remove_file(&body_path);
    let stderr_text = stderr_task.await.unwrap_or_default();
    if !status.success() {
        anyhow::bail!(
            "curl Cursor native API failed with status {}: {}",
            status,
            stderr_text.trim()
        );
    }

    if !pending.is_empty() {
        drain_native_frames(&tx, &mut pending, &mut router).await?;
    }
    for event in router.finish() {
        if tx.send(Ok(event)).await.is_err() {
            return Ok(());
        }
    }
    let _ = tx
        .send(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        }))
        .await;
    Ok(())
}

async fn drain_native_frames(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    pending: &mut Vec<u8>,
    router: &mut ThinkRouter,
) -> Result<()> {
    loop {
        let Some((frame_type, payload, consumed)) = decode_next_frame(pending)? else {
            break;
        };
        pending.drain(..consumed);
        match frame_type {
            0 | 1 => {
                for event in decode_protobuf_events(&payload, router)? {
                    if tx.send(Ok(event)).await.is_err() {
                        return Ok(());
                    }
                }
            }
            2 | 3 => {
                if payload != b"{}" {
                    let json: Value = serde_json::from_slice(&payload)
                        .context("Failed to decode Cursor native JSON frame")?;
                    if let Some(message) = json
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                    {
                        if message.eq_ignore_ascii_case("error") {
                            anyhow::bail!("Cursor native API stream error: {}", json);
                        }
                        anyhow::bail!("Cursor native API stream error: {}", message);
                    }
                    anyhow::bail!("Cursor native API stream error: {}", json);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn build_native_request_body(prompt: &str, model: &str) -> Vec<u8> {
    let message_id = Uuid::new_v4().to_string();
    let conversation_id = Uuid::new_v4().to_string();
    let request = {
        let mut bytes = Vec::new();
        bytes.extend(encode_field(
            1,
            2,
            encode_message(prompt, 1, &message_id, Some(1)),
        ));
        bytes.extend(encode_field(2, 0, encode_varint_bytes(1)));
        bytes.extend(encode_field(3, 2, Vec::<u8>::new()));
        bytes.extend(encode_field(4, 0, encode_varint_bytes(1)));
        bytes.extend(encode_field(5, 2, encode_model(model)));
        bytes.extend(encode_field(8, 2, Vec::<u8>::new()));
        bytes.extend(encode_field(13, 0, encode_varint_bytes(1)));
        bytes.extend(encode_field(15, 2, encode_cursor_setting()));
        bytes.extend(encode_field(19, 0, encode_varint_bytes(1)));
        bytes.extend(encode_field(23, 2, conversation_id.into_bytes()));
        bytes.extend(encode_field(26, 2, encode_metadata()));
        bytes.extend(encode_field(27, 0, encode_varint_bytes(0)));
        bytes.extend(encode_field(30, 2, encode_message_id(&message_id, 1)));
        bytes.extend(encode_field(35, 0, encode_varint_bytes(0)));
        bytes.extend(encode_field(38, 0, encode_varint_bytes(0)));
        bytes.extend(encode_field(46, 0, encode_varint_bytes(1)));
        bytes.extend(encode_field(47, 2, Vec::<u8>::new()));
        bytes.extend(encode_field(48, 0, encode_varint_bytes(0)));
        bytes.extend(encode_field(49, 0, encode_varint_bytes(0)));
        bytes.extend(encode_field(51, 0, encode_varint_bytes(0)));
        bytes.extend(encode_field(53, 0, encode_varint_bytes(1)));
        bytes.extend(encode_field(54, 2, b"Ask".to_vec()));
        bytes
    };

    let outer = encode_field(1, 2, request);
    let mut body = Vec::with_capacity(outer.len() + 5);
    body.push(0);
    body.extend_from_slice(&(outer.len() as u32).to_be_bytes());
    body.extend_from_slice(&outer);
    body
}

fn encode_message(
    content: &str,
    role: u64,
    message_id: &str,
    chat_mode_enum: Option<u64>,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(encode_field(1, 2, content.as_bytes().to_vec()));
    bytes.extend(encode_field(2, 0, encode_varint_bytes(role)));
    bytes.extend(encode_field(13, 2, message_id.as_bytes().to_vec()));
    if let Some(chat_mode_enum) = chat_mode_enum {
        bytes.extend(encode_field(47, 0, encode_varint_bytes(chat_mode_enum)));
    }
    bytes
}

fn encode_model(name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(encode_field(1, 2, name.as_bytes().to_vec()));
    bytes.extend(encode_field(4, 2, Vec::<u8>::new()));
    bytes
}

fn encode_cursor_setting() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(encode_field(1, 2, b"cursor\\aisettings".to_vec()));
    bytes.extend(encode_field(3, 2, Vec::<u8>::new()));
    let mut unknown6 = Vec::new();
    unknown6.extend(encode_field(1, 2, Vec::<u8>::new()));
    unknown6.extend(encode_field(2, 2, Vec::<u8>::new()));
    bytes.extend(encode_field(6, 2, unknown6));
    bytes.extend(encode_field(8, 0, encode_varint_bytes(1)));
    bytes.extend(encode_field(9, 0, encode_varint_bytes(1)));
    bytes
}

fn encode_metadata() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(encode_field(1, 2, std::env::consts::OS.as_bytes().to_vec()));
    bytes.extend(encode_field(
        2,
        2,
        std::env::consts::ARCH.as_bytes().to_vec(),
    ));
    bytes.extend(encode_field(
        3,
        2,
        std::env::consts::ARCH.as_bytes().to_vec(),
    ));
    bytes.extend(encode_field(4, 2, b"jcode".to_vec()));
    bytes.extend(encode_field(
        5,
        2,
        chrono::Utc::now().to_rfc3339().into_bytes(),
    ));
    bytes
}

fn encode_message_id(message_id: &str, role: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(encode_field(1, 2, message_id.as_bytes().to_vec()));
    bytes.extend(encode_field(3, 0, encode_varint_bytes(role)));
    bytes
}

fn encode_field(field_number: u64, wire_type: u8, value: Vec<u8>) -> Vec<u8> {
    let mut bytes = encode_varint_bytes((field_number << 3) | u64::from(wire_type));
    match wire_type {
        0 => bytes.extend(value),
        2 => {
            bytes.extend(encode_varint_bytes(value.len() as u64));
            bytes.extend(value);
        }
        _ => unreachable!("unsupported wire type"),
    }
    bytes
}

fn encode_varint_bytes(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 0x80 {
        bytes.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

fn decode_next_frame(buffer: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>> {
    if buffer.len() < 5 {
        return Ok(None);
    }
    let frame_type = buffer[0];
    let payload_len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    let consumed = 5 + payload_len;
    if buffer.len() < consumed {
        return Ok(None);
    }
    let payload = &buffer[5..consumed];
    let payload = match frame_type {
        1 | 3 => gunzip(payload)?,
        _ => payload.to_vec(),
    };
    Ok(Some((frame_type, payload, consumed)))
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .context("Failed to gunzip Cursor response payload")?;
    Ok(decoded)
}

fn decode_protobuf_events(payload: &[u8], router: &mut ThinkRouter) -> Result<Vec<StreamEvent>> {
    let mut events = Vec::new();
    for field in parse_fields(payload)? {
        if field.number == 2
            && let FieldValue::Bytes(inner) = field.value
            && let Ok(inner_fields) = parse_fields(&inner)
        {
            for inner_field in inner_fields {
                if inner_field.number == 1
                    && let FieldValue::Bytes(text) = inner_field.value
                {
                    let text = String::from_utf8_lossy(&text);
                    events.extend(router.push_chunk(&text));
                }
            }
        }
    }
    Ok(events)
}

#[derive(Debug)]
struct ProtobufField {
    number: u64,
    value: FieldValue,
}

enum FieldValue {
    Varint(u64),
    Bytes(Vec<u8>),
    Fixed32([u8; 4]),
    Fixed64([u8; 8]),
}

impl fmt::Debug for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Varint(value) => f.debug_tuple("Varint").field(value).finish(),
            Self::Bytes(bytes) => f.debug_struct("Bytes").field("len", &bytes.len()).finish(),
            Self::Fixed32(bytes) => f.debug_tuple("Fixed32").field(bytes).finish(),
            Self::Fixed64(bytes) => f.debug_tuple("Fixed64").field(bytes).finish(),
        }
    }
}

fn parse_fields(bytes: &[u8]) -> Result<Vec<ProtobufField>> {
    let mut fields = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let tag = decode_varint(bytes, &mut index)?;
        let number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        let value = match wire_type {
            0 => FieldValue::Varint(decode_varint(bytes, &mut index)?),
            1 => {
                let end = index + 8;
                let slice = bytes
                    .get(index..end)
                    .ok_or_else(|| anyhow::anyhow!("Truncated fixed64 protobuf field"))?;
                index = end;
                let mut array = [0u8; 8];
                array.copy_from_slice(slice);
                FieldValue::Fixed64(array)
            }
            2 => {
                let len = decode_varint(bytes, &mut index)? as usize;
                let end = index + len;
                let slice = bytes
                    .get(index..end)
                    .ok_or_else(|| anyhow::anyhow!("Truncated length-delimited protobuf field"))?;
                index = end;
                FieldValue::Bytes(slice.to_vec())
            }
            5 => {
                let end = index + 4;
                let slice = bytes
                    .get(index..end)
                    .ok_or_else(|| anyhow::anyhow!("Truncated fixed32 protobuf field"))?;
                index = end;
                let mut array = [0u8; 4];
                array.copy_from_slice(slice);
                FieldValue::Fixed32(array)
            }
            _ => anyhow::bail!("Unsupported protobuf wire type {}", wire_type),
        };
        fields.push(ProtobufField { number, value });
    }
    Ok(fields)
}

fn decode_varint(bytes: &[u8], index: &mut usize) -> Result<u64> {
    let mut shift = 0u32;
    let mut value = 0u64;
    loop {
        let byte = *bytes
            .get(*index)
            .ok_or_else(|| anyhow::anyhow!("Unexpected EOF while decoding protobuf varint"))?;
        *index += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            anyhow::bail!("Protobuf varint too large");
        }
    }
}

#[derive(Default)]
struct ThinkRouter {
    in_think: bool,
    carry: String,
}

impl ThinkRouter {
    fn push_chunk(&mut self, chunk: &str) -> Vec<StreamEvent> {
        self.route(Some(chunk))
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        self.route(None)
    }

    fn route(&mut self, chunk: Option<&str>) -> Vec<StreamEvent> {
        if let Some(chunk) = chunk {
            self.carry.push_str(chunk);
        }
        let mut events = Vec::new();
        loop {
            if self.in_think {
                if let Some(idx) = self.carry.find("</think>") {
                    let text = self.carry[..idx].to_string();
                    if !text.is_empty() {
                        events.push(StreamEvent::ThinkingDelta(text));
                    }
                    events.push(StreamEvent::ThinkingEnd);
                    self.carry = self.carry[idx + "</think>".len()..].to_string();
                    self.in_think = false;
                    continue;
                }
                let split = carry_boundary(&self.carry, "</think>");
                if split > 0 {
                    let text = self.carry[..split].to_string();
                    if !text.is_empty() {
                        events.push(StreamEvent::ThinkingDelta(text));
                    }
                    self.carry = self.carry[split..].to_string();
                }
                break;
            }

            if let Some(idx) = self.carry.find("<think>") {
                let text = self.carry[..idx].to_string();
                if !text.is_empty() {
                    events.push(StreamEvent::TextDelta(text));
                }
                events.push(StreamEvent::ThinkingStart);
                self.carry = self.carry[idx + "<think>".len()..].to_string();
                self.in_think = true;
                continue;
            }

            let split = carry_boundary(&self.carry, "<think>");
            if split > 0 {
                let text = self.carry[..split].to_string();
                if !text.is_empty() {
                    events.push(StreamEvent::TextDelta(text));
                }
                self.carry = self.carry[split..].to_string();
            }
            break;
        }

        if chunk.is_none() && !self.carry.is_empty() {
            if self.in_think {
                events.push(StreamEvent::ThinkingDelta(std::mem::take(&mut self.carry)));
                events.push(StreamEvent::ThinkingEnd);
                self.in_think = false;
            } else {
                events.push(StreamEvent::TextDelta(std::mem::take(&mut self.carry)));
            }
        }

        events
    }
}

fn carry_boundary(text: &str, marker: &str) -> usize {
    let max = marker.len().saturating_sub(1).min(text.len());
    for keep in (1..=max).rev() {
        if text.ends_with(&marker[..keep]) {
            return text.len() - keep;
        }
    }
    text.len()
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod cursor_tests;
