use super::cli_common::{build_cli_prompt, run_cli_text_command};
use super::{EventStream, Provider};
use crate::auth::cursor as cursor_auth;
use crate::message::{Message, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::io::Read;
use std::sync::{Arc, RwLock};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

const DIRECT_CHAT_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.ChatService/StreamUnifiedChatWithTools";
const DEFAULT_MODEL: &str = "composer-1.5";
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

fn runtime_cursor_api_key() -> Option<String> {
    crate::auth::cursor::load_api_key().ok()
}

fn use_native_transport() -> bool {
    match std::env::var("JCODE_CURSOR_TRANSPORT") {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "cli" => false,
            "native" | "direct" | "http" | "https" | "connect" => true,
            _ => cursor_auth::has_cursor_native_auth(),
        },
        Err(_) => cursor_auth::has_cursor_native_auth(),
    }
}

pub struct CursorCliProvider {
    cli_path: String,
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
}

impl CursorCliProvider {
    pub fn new() -> Self {
        let cli_path =
            std::env::var("JCODE_CURSOR_CLI_PATH").unwrap_or_else(|_| "cursor-agent".to_string());
        let model = std::env::var("JCODE_CURSOR_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Self {
            cli_path,
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
        }
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
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_cli_prompt(system, messages);
        let model = self.model.read().unwrap().clone();
        let cli_path = self.cli_path.clone();
        let client = self.client.clone();
        let api_key = runtime_cursor_api_key();
        let resume = resume_session_id.map(|s| s.to_string());
        let cwd = std::env::current_dir().ok();
        let (tx, rx) = mpsc::channel::<Result<crate::message::StreamEvent>>(100);

        tokio::spawn(async move {
            let result = if use_native_transport() {
                run_native_text_command(client, tx.clone(), &prompt, &model).await
            } else {
                run_cli_command(cli_path, api_key, cwd, resume, model, prompt, tx.clone()).await
            };

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
        self.model.read().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Cursor model cannot be empty");
        }
        *self.model.write().unwrap() = trimmed.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn handles_tools_internally(&self) -> bool {
        !use_native_transport()
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            cli_path: self.cli_path.clone(),
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
        })
    }
}

async fn run_cli_command(
    cli_path: String,
    api_key: Option<String>,
    cwd: Option<std::path::PathBuf>,
    resume: Option<String>,
    model: String,
    prompt: String,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    if tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "cli subprocess".to_string(),
        }))
        .await
        .is_err()
    {
        return Ok(());
    }

    let mut cmd = Command::new(&cli_path);
    cmd.arg("-p")
        .arg("--print")
        .arg("--output-format")
        .arg("text")
        .arg("--model")
        .arg(&model);
    if let Some(ref session_id) = resume {
        cmd.arg("--resume").arg(session_id);
    }
    cmd.arg(prompt);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if let Some(api_key) = api_key {
        cmd.env("CURSOR_API_KEY", api_key);
    }

    run_cli_text_command(cmd, tx, "Cursor").await
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
    let session_id = cursor_auth::session_id_for_access_token(&tokens.access_token);
    let request_id = Uuid::new_v4().to_string();
    let config_version = Uuid::new_v4().to_string();
    let body = build_native_request_body(prompt, model);

    run_native_text_command_via_curl(
        tx,
        &tokens.access_token,
        &session_id,
        &request_id,
        &config_version,
        body,
    )
    .await
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
                        anyhow::bail!("Cursor native API stream error: {}", message);
                    }
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
        if field.number == 2 {
            if let FieldValue::Bytes(inner) = field.value {
                if let Ok(inner_fields) = parse_fields(&inner) {
                    for inner_field in inner_fields {
                        if inner_field.number == 1 {
                            if let FieldValue::Bytes(text) = inner_field.value {
                                let text = String::from_utf8_lossy(&text);
                                events.extend(router.push_chunk(&text));
                            }
                        }
                    }
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

#[derive(Debug)]
enum FieldValue {
    Varint(u64),
    Bytes(Vec<u8>),
    Fixed32([u8; 4]),
    Fixed64([u8; 8]),
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

        if chunk.is_none() {
            if !self.carry.is_empty() {
                if self.in_think {
                    events.push(StreamEvent::ThinkingDelta(std::mem::take(&mut self.carry)));
                    events.push(StreamEvent::ThinkingEnd);
                    self.in_think = false;
                } else {
                    events.push(StreamEvent::TextDelta(std::mem::take(&mut self.carry)));
                }
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
mod tests {
    use super::*;

    #[test]
    fn available_models_include_composer_models() {
        let provider = CursorCliProvider::new();
        let models = provider.available_models();
        assert!(models.contains(&"composer-1"));
        assert!(models.contains(&"composer-1.5"));
    }

    #[test]
    fn set_model_accepts_composer_models() {
        let provider = CursorCliProvider::new();

        provider.set_model("composer-1").unwrap();
        assert_eq!(provider.model(), "composer-1");

        provider.set_model("composer-1.5").unwrap();
        assert_eq!(provider.model(), "composer-1.5");
    }

    #[test]
    fn runtime_cursor_api_key_reads_env() {
        let previous = std::env::var_os("CURSOR_API_KEY");
        crate::env::set_var("CURSOR_API_KEY", "cursor-env-test");

        assert_eq!(runtime_cursor_api_key().as_deref(), Some("cursor-env-test"));

        if let Some(previous) = previous {
            crate::env::set_var("CURSOR_API_KEY", previous);
        } else {
            crate::env::remove_var("CURSOR_API_KEY");
        }
    }

    #[test]
    fn think_router_splits_reasoning_and_text() {
        let mut router = ThinkRouter::default();
        let events = router.push_chunk("hello<think>secret</think>world");
        assert!(matches!(events[0], StreamEvent::TextDelta(_)));
        assert!(matches!(events[1], StreamEvent::ThinkingStart));
        assert!(matches!(events[2], StreamEvent::ThinkingDelta(_)));
        assert!(matches!(events[3], StreamEvent::ThinkingEnd));
        assert!(matches!(events[4], StreamEvent::TextDelta(_)));
    }
}
