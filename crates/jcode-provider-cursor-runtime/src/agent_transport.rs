//! Native Cursor Agent transport implementing `agent.v1.AgentService/Run`.
//!
//! Cursor decommissioned the old `api2.cursor.sh/aiserver.v1.ChatService/
//! StreamUnifiedChatWithTools` endpoint for API-key / CLI tokens (it now returns
//! `resource_exhausted` "Update Required" / `actionRequired: payment`). The
//! current, working transport used by the `cursor-agent` CLI is a *paced,
//! bidirectional* Connect-over-HTTP/2 stream against
//! `agentn.global.api5.cursor.sh/agent.v1.AgentService/Run`.
//!
//! Wire format (reverse-engineered by MITM-capturing the real `cursor-agent`):
//!
//! * Connect streaming framing: each message is `[1 flag byte][4-byte BE len]
//!   [payload]`. Flag `0x01` = payload gzip-compressed, `0x02` = end-of-stream
//!   trailer (JSON, `{}` on success or `{"error":...}`).
//! * The logical `RunInput` is split across several request frames, each
//!   carrying a different top-level protobuf field:
//!   - frame 0 = field 1 (`RunRequest`: prompt, model, model catalog),
//!   - frame 1 = field 2 (environment/tool context),
//!   - then a short sequence of small field-3/5/7 marker frames.
//! * The client keeps the request stream **open** while reading the response,
//!   emitting periodic `f7:''` heartbeats (~5s) and pacing marker frames as the
//!   server streams, half-closing only after the server completes. Sending the
//!   whole body then immediately half-closing yields only keepalives / an
//!   `internal: No exec result` error, so the pacing is load-bearing.
//!
//! Response text arrives as `f1.f1.f1` string chunks (assistant answer) and
//! `f1.f4.f1` chunks (reasoning). A trailing flag-`0x02` frame closes the turn.

use std::io::Read;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use flate2::read::GzDecoder;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at};
use uuid::Uuid;

use jcode_message_types::StreamEvent;

const AGENT_HOST: &str = "agentn.global.api5.cursor.sh";
const AGENT_PATH: &str = "/agent.v1.AgentService/Run";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// Client version advertised to Cursor's agent service. Must track a currently
/// served `cursor-agent` CLI build; override at runtime with
/// `JCODE_CURSOR_CLI_VERSION` if Cursor moves the floor.
const CLI_CLIENT_VERSION_DEFAULT: &str = "cli-2026.08.25-3e8eec8";

pub(crate) fn cli_client_version() -> String {
    std::env::var("JCODE_CURSOR_CLI_VERSION")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
        .unwrap_or_else(|| CLI_CLIENT_VERSION_DEFAULT.to_string())
}

/// Extract a bare host from a value that may be a full URL, a host with a
/// scheme, or a host with trailing path/port noise.
///
/// The result is used directly as a DNS name (`TcpStream::connect`) and as the
/// TLS `ServerName`, so it has to be a bare host: a leftover `:443` suffix or a
/// trailing path makes both of those fail.
fn normalize_agent_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let raw = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(raw);
    let host = raw.split('/').next().unwrap_or_default().trim();
    // Drop any explicit port. Connections are always made on 443, so a port in
    // the cached or overridden value is noise that would otherwise land inside
    // the DNS name and the TLS SNI value.
    let host = host.split(':').next().unwrap_or(host).trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Regional agent host cached by the official `cursor-agent` CLI.
///
/// `cursor-agent` bootstraps `GetServerConfig` and caches the endpoint its team
/// is actually routed to. Teams pinned to a region reject the `global` host with
/// "This region is not yet available for your team" (issue #637), so reuse the
/// CLI's cached value when it is present.
fn agent_host_from_cursor_cli_config() -> Option<String> {
    let path = match jcode_base::storage::user_home_path(".cursor/cli-config.json") {
        Ok(path) => path,
        Err(err) => {
            jcode_base::logging::warn(&format!(
                "Cursor: cannot locate ~/.cursor/cli-config.json ({err}); \
                 falling back to the global agent host"
            ));
            return None;
        }
    };
    // A missing file is the normal case for users who never ran `cursor-agent`,
    // so that is not worth warning about. Anything else (unreadable, malformed,
    // unexpected shape) is worth surfacing: silently discarding it is what made
    // issue #637 hard to diagnose in the first place.
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            jcode_base::logging::warn(&format!(
                "Cursor: cannot read {} ({err}); falling back to the global agent host",
                path.display()
            ));
            return None;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(err) => {
            jcode_base::logging::warn(&format!(
                "Cursor: {} is not valid JSON ({err}); falling back to the global agent host",
                path.display()
            ));
            return None;
        }
    };
    let Some(url_config) = value
        .get("serverConfigCache")
        .and_then(|cache| cache.get("agentUrlConfig"))
    else {
        jcode_base::logging::warn(&format!(
            "Cursor: {} has no serverConfigCache.agentUrlConfig; \
             falling back to the global agent host. Running `cursor-agent status` \
             usually repopulates it.",
            path.display()
        ));
        return None;
    };
    let candidate = url_config
        .get("agentnUrl")
        .and_then(|v| v.as_str())
        .or_else(|| url_config.get("agentUrl").and_then(|v| v.as_str()));
    let Some(host) = candidate.and_then(normalize_agent_host) else {
        jcode_base::logging::warn(&format!(
            "Cursor: {} has no usable agentnUrl/agentUrl host (found {candidate:?}); \
             falling back to the global agent host",
            path.display()
        ));
        return None;
    };
    jcode_base::logging::info(&format!(
        "Cursor: using regional agent host {host} from {}",
        path.display()
    ));
    Some(host)
}

/// Resolve the Cursor agent host, in precedence order:
/// 1. `JCODE_CURSOR_AGENT_HOST` / `CURSOR_AGENT_HOST` (explicit override)
/// 2. `~/.cursor/cli-config.json` regional endpoint cached by `cursor-agent`
/// 3. the `global` host, as a last-resort fallback
pub(crate) fn agent_host() -> String {
    for var in ["JCODE_CURSOR_AGENT_HOST", "CURSOR_AGENT_HOST"] {
        if let Ok(raw) = std::env::var(var) {
            // Normalize overrides too. These get copied straight out of
            // `cli-config.json` or a browser, so `https://host/path` and
            // `host:443` are both likely spellings and both have to work.
            if let Some(host) = normalize_agent_host(&raw) {
                return host;
            }
        }
    }
    agent_host_from_cursor_cli_config().unwrap_or_else(|| AGENT_HOST.to_string())
}

// --------------------------------------------------------------------------
// Protobuf + Connect framing helpers
// --------------------------------------------------------------------------

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Encode a length-delimited (wire type 2) protobuf field.
fn field_ld(field: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    encode_varint((field << 3) | 2, &mut out);
    encode_varint(data.len() as u64, &mut out);
    out.extend_from_slice(data);
    out
}

/// Encode a varint (wire type 0) protobuf field.
fn field_varint(field: u64, value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    encode_varint(field << 3, &mut out);
    encode_varint(value, &mut out);
    out
}

fn field_str(field: u64, s: &str) -> Vec<u8> {
    field_ld(field, s.as_bytes())
}

/// Wrap a protobuf message payload in a Connect data frame using the same
/// threshold as connect-es. Large tool results and context messages must carry
/// the gzip flag when the request advertises gzip content encoding.
fn connect_frame(payload: &[u8]) -> Vec<u8> {
    crate::wire::connect_frame(payload)
}

/// `{f1: name, f3: {f1:'fast', f2:'true'|'false'}}` model descriptor.
fn encode_model_meta(name: &str, fast: bool) -> Vec<u8> {
    let mut out = field_str(1, name);
    let mut kv = field_str(1, "fast");
    kv.extend(field_str(2, if fast { "true" } else { "false" }));
    out.extend(field_ld(3, &kv));
    out
}

/// Build the request frames for a single-shot prompt turn.
///
/// Returns the initial Connect frame for the streamed `RunInput`.
///
/// The current connect-es client sends only `AgentRunRequest` up front. The
/// server requests environment context and other execution messages on the
/// same bidirectional stream later. Sending the old captured context and marker
/// frames proactively is tolerated for plain chat but causes current Cursor
/// tool turns to remain in an unacknowledged state.
fn build_run_frames(
    prompt: &str,
    model: &str,
    _cwd: &str,
    tools: &[jcode_message_types::ToolDefinition],
    request_id: &str,
) -> Vec<Vec<u8>> {
    let conv = Uuid::new_v4().to_string();
    let msg = Uuid::new_v4().to_string();

    // frame 0: field 1 = RunRequest
    // messages: f2 { f1 { f1 { f1:prompt, f2:msg_id, f3:'', f4:1 } } }
    let mut inner = field_str(1, prompt);
    inner.extend(field_str(2, &msg));
    inner.extend(field_str(3, ""));
    inner.extend(field_varint(4, 1));
    let messages = field_ld(2, &field_ld(1, &field_ld(1, &inner)));

    let mut req = field_str(1, "");
    req.extend(messages);
    if !tools.is_empty() {
        if let Ok(mcp_tools_bytes) = crate::wire::encode_mcp_tools(tools) {
            req.extend(field_ld(4, &mcp_tools_bytes));
        }
    } else {
        req.extend(field_str(4, ""));
    }
    req.extend(field_str(5, &conv));
    req.extend(field_ld(9, &encode_model_meta(model, false)));
    req.extend(field_varint(12, 0));
    // minimal catalog: a "default" entry plus the target model
    req.extend(field_ld(14, &field_str(1, "default")));
    req.extend(field_ld(14, &encode_model_meta(model, false)));
    req.extend(field_str(16, &conv));
    req.extend(field_str(25, request_id));
    let frame0 = connect_frame(&field_ld(1, &req));

    vec![frame0]
}

/// A single `f7:''` heartbeat frame.
fn heartbeat_frame() -> Vec<u8> {
    connect_frame(&field_ld(7, &[]))
}

// --------------------------------------------------------------------------
// Response parsing
// --------------------------------------------------------------------------

/// Incrementally decode Connect frames from a byte buffer, returning
/// `(flag, payload, consumed)` for the next complete frame or `None`.
fn next_frame(buf: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
    if buf.len() < 5 {
        return None;
    }
    let flag = buf[0];
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    let end = 5 + len;
    if buf.len() < end {
        return None;
    }
    let mut payload = buf[5..end].to_vec();
    if flag & 0x01 != 0 {
        // gzip-compressed payload
        let mut decoded = Vec::new();
        if GzDecoder::new(&payload[..])
            .read_to_end(&mut decoded)
            .is_ok()
        {
            payload = decoded;
        }
    }
    Some((flag, payload, end))
}

/// Minimal protobuf reader that extracts assistant text chunks from a response
/// message. Text answer chunks live at `f1.f1.f1` (string); reasoning chunks at
/// `f1.f4.f1` (string). We only surface the assistant answer to keep the stream
/// clean, matching the old provider's text-only behavior.
struct PbField<'a> {
    field: u64,
    wire: u8,
    data: &'a [u8],
}

fn iter_fields(mut buf: &[u8]) -> impl Iterator<Item = PbField<'_>> {
    std::iter::from_fn(move || {
        if buf.is_empty() {
            return None;
        }
        let (tag, rest) = read_varint(buf)?;
        let field = tag >> 3;
        let wire = (tag & 7) as u8;
        buf = rest;
        match wire {
            0 => {
                let (_v, rest) = read_varint(buf)?;
                buf = rest;
                Some(PbField {
                    field,
                    wire,
                    data: &[],
                })
            }
            2 => {
                let (len, rest) = read_varint(buf)?;
                let len = len as usize;
                if rest.len() < len {
                    return None;
                }
                let data = &rest[..len];
                buf = &rest[len..];
                Some(PbField { field, wire, data })
            }
            5 => {
                if buf.len() < 4 {
                    return None;
                }
                buf = &buf[4..];
                Some(PbField {
                    field,
                    wire,
                    data: &[],
                })
            }
            1 => {
                if buf.len() < 8 {
                    return None;
                }
                buf = &buf[8..];
                Some(PbField {
                    field,
                    wire,
                    data: &[],
                })
            }
            _ => None,
        }
    })
}

fn read_varint(buf: &[u8]) -> Option<(u64, &[u8])> {
    let mut result = 0u64;
    let mut shift = 0u32;
    for (i, &byte) in buf.iter().enumerate() {
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, &buf[i + 1..]));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// Extract the assistant answer text delta from one response message payload.
///
/// The assistant-answer chunk shape is `f1 { f1 { f1: <str> } }`. We ignore
/// reasoning (`f1.f4`) so the emitted stream matches plain chat text.
fn extract_answer_text(payload: &[u8]) -> Option<String> {
    for f1 in iter_fields(payload) {
        if f1.field != 1 || f1.wire != 2 {
            continue;
        }
        for f1_1 in iter_fields(f1.data) {
            if f1_1.field != 1 || f1_1.wire != 2 {
                continue;
            }
            for leaf in iter_fields(f1_1.data) {
                if leaf.field == 1
                    && leaf.wire == 2
                    && let Ok(s) = std::str::from_utf8(leaf.data)
                    && !s.is_empty()
                {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// Extract reasoning delta (thinking text) from one response message payload.
/// The reasoning chunk shape is `f1 { f4 { f1: <str> } }`.
pub(crate) fn extract_thinking_text(payload: &[u8]) -> Option<String> {
    for f1 in iter_fields(payload) {
        if f1.field != 1 || f1.wire != 2 {
            continue;
        }
        for f4 in iter_fields(f1.data) {
            if f4.field != 4 || f4.wire != 2 {
                continue;
            }
            for leaf in iter_fields(f4.data) {
                if leaf.field == 1
                    && leaf.wire == 2
                    && let Ok(s) = std::str::from_utf8(leaf.data)
                    && !s.is_empty()
                {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

// --------------------------------------------------------------------------
// TLS + HTTP/2 bidirectional client
// --------------------------------------------------------------------------

fn tls_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}

pub(crate) fn parse_tool_request_id(request_id: &str) -> (u32, &str) {
    let mut parts = request_id.splitn(3, ':');
    let _stream_uuid = parts.next();
    let id = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(1);
    (id, parts.next().unwrap_or(""))
}

/// Run one Cursor agent turn and forward assistant text as [`StreamEvent`]s.
pub async fn run_agent_turn(
    access_token: &str,
    prompt: &str,
    model: &str,
    logical_session_id: Option<&str>,
    stream_uuid: &str,
    tools: &[jcode_message_types::ToolDefinition],
    system: &str,
    tool_result_rx: &mut mpsc::Receiver<jcode_provider_core::NativeToolResult>,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    use h2::client;
    use http::{Method, Request};
    use tokio_rustls::TlsConnector;

    let host = agent_host();
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "/".to_string());

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "native http2 (agent)".to_string(),
        }))
        .await;

    // Establish TLS + HTTP/2.
    let tcp = tokio::net::TcpStream::connect((host.as_str(), 443))
        .await
        .with_context(|| format!("Failed to connect to {host}:443"))?;
    tcp.set_nodelay(true).ok();
    let connector = TlsConnector::from(tls_config());
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.clone())
        .context("Invalid Cursor agent host name")?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .context("TLS handshake with Cursor agent host failed")?;

    let (h2, connection) = client::handshake(tls)
        .await
        .context("HTTP/2 handshake with Cursor agent host failed")?;
    // Drive the connection in the background.
    let conn_task = tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut h2 = h2.ready().await.context("HTTP/2 connection not ready")?;

    let request_id = Uuid::new_v4().to_string();
    let session_id = logical_session_id
        .map(|id| Uuid::new_v5(&Uuid::NAMESPACE_DNS, id.as_bytes()))
        .unwrap_or_else(Uuid::new_v4)
        .to_string();
    let traceparent = format!(
        "00-{}-{}-01",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple().to_string()[..16].to_string()
    );
    let blob_encryption_key = Uuid::new_v4().simple().to_string();
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("https://{host}{AGENT_PATH}"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("connect-accept-encoding", "gzip,br")
        .header("connect-content-encoding", "gzip")
        .header("connect-protocol-version", "1")
        .header("te", "trailers")
        .header("content-type", "application/connect+proto")
        .header("backend-traceparent", &traceparent)
        .header("traceparent", &traceparent)
        .header("user-agent", "connect-es/1.6.1")
        .header("x-blob-encryption-key", blob_encryption_key)
        .header("x-cursor-client-type", "cli")
        .header("x-cursor-client-version", cli_client_version())
        .header("x-ghost-mode", "true")
        .header("x-request-id", &request_id)
        .header("x-original-request-id", &request_id)
        .header("x-session-id", &session_id)
        .body(())
        .context("Failed to build Cursor agent request")?;

    let (response, mut send_stream) = h2
        .send_request(request, false)
        .context("Failed to send Cursor agent request headers")?;

    let _ = tx.send(Ok(StreamEvent::SessionId(session_id))).await;

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Vec<u8>>(32);
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

    let frames = build_run_frames(prompt, model, &cwd, tools, &request_id);
    let sender = tokio::spawn(async move {
        for (idx, frame) in frames.into_iter().enumerate() {
            if send_stream.send_data(Bytes::from(frame), false).is_err() {
                return;
            }
            // Drain any pending outbound frames between initial pace delays
            while let Ok(outbound_frame) = outbound_rx.try_recv() {
                if send_stream
                    .send_data(Bytes::from(outbound_frame), false)
                    .is_err()
                {
                    return;
                }
            }
            let pace = match idx {
                0 => Duration::from_millis(1500),
                1 => Duration::from_millis(800),
                _ => Duration::from_millis(400),
            };
            tokio::select! {
                _ = tokio::time::sleep(pace) => {}
                Some(outbound_frame) = outbound_rx.recv() => {
                    if send_stream.send_data(Bytes::from(outbound_frame), false).is_err() {
                        return;
                    }
                }
            }
        }
        let mut ticker = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                Some(frame) = outbound_rx.recv() => {
                    if send_stream.send_data(Bytes::from(frame), false).is_err() {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if send_stream.send_data(Bytes::from(heartbeat_frame()), false).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = send_stream.send_data(Bytes::new(), true);
    });

    // Receiver: read response body frames and forward assistant text.
    let response = response
        .await
        .context("Cursor agent request failed before response headers")?;
    let status = response.status();
    let mut body = response.into_body();
    let mut pending: Vec<u8> = Vec::new();
    let mut error_message: Option<String> = None;
    let mut got_text = false;
    let mut in_thinking = false;
    let mut active_tool_calls: usize = 0;
    let mut blob_store: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();

    // Idle timeouts guard against the server holding the stream open. Cursor
    // keeps the response side open after the assistant message when it expects
    // a tool exec-result, so we
    // finish the turn once output goes quiet. The first-byte budget is longer
    // because generation can take a few seconds to start.
    let first_byte_timeout = Duration::from_secs(60);
    let idle_timeout = Duration::from_secs(4);
    let tool_exec_timeout = Duration::from_secs(300);

    'read: loop {
        let budget = if active_tool_calls > 0 {
            tool_exec_timeout
        } else if got_text {
            idle_timeout
        } else {
            first_byte_timeout
        };
        let next = tokio::select! {
            res = tokio::time::timeout(budget, body.data()) => {
                match res {
                    Ok(Some(chunk)) => chunk,
                    Ok(None) | Err(_) => break 'read,
                }
            }
            Some(tool_res) = tool_result_rx.recv() => {
                let (id_u32, exec_id) = parse_tool_request_id(&tool_res.request_id);
                let raw_content = if tool_res.is_error {
                    tool_res.result.error.unwrap_or_default()
                } else {
                    tool_res.result.output.unwrap_or_default()
                };
                let redacted = jcode_base::message::redact_secrets(&raw_content);
                let mcp_res_bytes = crate::wire::encode_mcp_success_result(
                    id_u32,
                    exec_id,
                    &redacted,
                    tool_res.is_error,
                );
                let agent_client_bytes = crate::wire::encode_agent_client_exec_message(&mcp_res_bytes);
                let connect_bytes = crate::wire::connect_frame(&agent_client_bytes);
                let _ = outbound_tx.send(connect_bytes).await;
                if active_tool_calls > 0 {
                    active_tool_calls -= 1;
                }
                got_text = false;
                continue 'read;
            }
        };
        let chunk = next.context("Cursor agent response stream error")?;
        let _ = body.flow_control().release_capacity(chunk.len());
        pending.extend_from_slice(&chunk);
        while let Some((flag, payload, consumed)) = next_frame(&pending) {
            pending.drain(..consumed);
            if flag & 0x02 != 0 {
                // end-of-stream trailer (JSON). Detect errors, then finish.
                if let Ok(text) = std::str::from_utf8(&payload)
                    && let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
                    && let Some(err) = json.get("error")
                {
                    error_message = Some(err.to_string());
                }
                break 'read;
            }
            if let Some(text) = extract_answer_text(&payload) {
                if in_thinking {
                    let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                    in_thinking = false;
                }
                got_text = true;
                if tx.send(Ok(StreamEvent::TextDelta(text))).await.is_err() {
                    break 'read;
                }
                continue;
            }
            if let Some(thinking) = extract_thinking_text(&payload) {
                if !in_thinking {
                    let _ = tx.send(Ok(StreamEvent::ThinkingStart)).await;
                    in_thinking = true;
                }
                if tx
                    .send(Ok(StreamEvent::ThinkingDelta(thinking)))
                    .await
                    .is_err()
                {
                    break 'read;
                }
                continue;
            }
            if let Some(used_tokens) = crate::wire::extract_checkpoint_used_tokens(&payload) {
                let _ = tx
                    .send(Ok(StreamEvent::TokenUsage {
                        input_tokens: Some(used_tokens),
                        output_tokens: Some(0),
                        cache_read_input_tokens: None,
                        cache_creation_input_tokens: None,
                    }))
                    .await;
                continue;
            }
            // KV server messages are emitted throughout a turn, including
            // ordinary chat. They must be acknowledged or Cursor keeps the
            // bidirectional stream alive indefinitely.
            let mut kv_server_data: Option<&[u8]> = None;
            for f in crate::wire::iter_fields(&payload) {
                if f.field == 4 && f.wire == 2 {
                    kv_server_data = Some(f.data);
                    break;
                }
            }
            if let Some(data) = kv_server_data {
                let mut kv_id = 0u32;
                let mut get_blob_id: Option<Vec<u8>> = None;
                let mut set_blob: Option<(Vec<u8>, Vec<u8>)> = None;
                for field in crate::wire::iter_fields(data) {
                    match field.field {
                        1 if field.wire == 0 => kv_id = field.varint as u32,
                        2 if field.wire == 2 => {
                            let id = crate::wire::iter_fields(field.data)
                                .find(|f| f.field == 1 && f.wire == 2)
                                .map(|f| f.data.to_vec());
                            get_blob_id = id;
                        }
                        3 if field.wire == 2 => {
                            let mut id = None;
                            let mut blob = None;
                            for f in crate::wire::iter_fields(field.data) {
                                if f.field == 1 && f.wire == 2 {
                                    id = Some(f.data.to_vec());
                                } else if f.field == 2 && f.wire == 2 {
                                    blob = Some(f.data.to_vec());
                                }
                            }
                            if let (Some(id), Some(blob)) = (id, blob) {
                                set_blob = Some((id, blob));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some((id, blob)) = set_blob {
                    blob_store.insert(id, blob);
                    let _ = outbound_tx
                        .send(crate::wire::connect_frame(
                            &crate::wire::encode_kv_set_blob_ack(kv_id),
                        ))
                        .await;
                } else if let Some(id) = get_blob_id {
                    let blob = blob_store.get(&id).map(Vec::as_slice).unwrap_or(&[]);
                    let _ = outbound_tx
                        .send(crate::wire::connect_frame(
                            &crate::wire::encode_kv_get_blob_result(kv_id, blob),
                        ))
                        .await;
                }
                continue;
            }

            // Check for ExecServerMessage (field 2 of AgentServerMessage)
            let mut exec_server_data: Option<&[u8]> = None;
            for f in crate::wire::iter_fields(&payload) {
                if f.field == 2 && f.wire == 2 {
                    exec_server_data = Some(f.data);
                    break;
                }
            }
            if let Some(data) = exec_server_data {
                if let Ok(msg) = crate::wire::decode_exec_server_message(data) {
                    use crate::wire::ExecServerMessageVariant;
                    match msg.variant {
                        ExecServerMessageVariant::Mcp(mcp_args) => {
                            if in_thinking {
                                let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                                in_thinking = false;
                            }
                            let bare_tool_name = crate::wire::mcp_bare_name(&mcp_args.name);
                            let corr_request_id =
                                format!("{}:{}:{}", stream_uuid, msg.id, msg.exec_id);
                            active_tool_calls += 1;
                            if tx
                                .send(Ok(StreamEvent::NativeToolCall {
                                    request_id: corr_request_id,
                                    tool_name: bare_tool_name.to_string(),
                                    input: mcp_args.args,
                                }))
                                .await
                                .is_err()
                            {
                                break 'read;
                            }
                        }
                        ExecServerMessageVariant::RequestContext(_) => {
                            let rc_bytes = crate::wire::encode_request_context(system, &cwd)
                                .unwrap_or_default();
                            let res_bytes = crate::wire::encode_request_context_result(
                                msg.id,
                                &msg.exec_id,
                                &rc_bytes,
                            );
                            let agent_bytes =
                                crate::wire::encode_agent_client_exec_message(&res_bytes);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Shell(args) => {
                            let rej = crate::wire::reject_shell_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.command,
                                &args.working_directory,
                                crate::wire::native_shell_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Write(args) => {
                            let rej = crate::wire::reject_write_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.path,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Delete(args) => {
                            let rej = crate::wire::reject_delete_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.path,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Grep(_) => {
                            let rej = crate::wire::reject_grep_exec(
                                msg.id,
                                &msg.exec_id,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Read(args) => {
                            let rej = crate::wire::reject_read_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.path,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Ls(args) => {
                            let rej = crate::wire::reject_ls_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.path,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Diagnostics(args) => {
                            let rej = crate::wire::reject_diagnostics_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.path,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::ShellStream(_) => {
                            let rej = crate::wire::reject_shell_stream_exec(
                                msg.id,
                                &msg.exec_id,
                                crate::wire::native_shell_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::BackgroundShellSpawn(args) => {
                            let rej = crate::wire::reject_background_shell_spawn_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.command,
                                &args.working_directory,
                                crate::wire::native_shell_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::ListMcpResources(_) => {
                            let rej = crate::wire::reject_list_mcp_resources_exec(
                                msg.id,
                                &msg.exec_id,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::ReadMcpResource(args) => {
                            let rej = crate::wire::reject_read_mcp_resource_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.uri,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Fetch(args) => {
                            let rej = crate::wire::reject_fetch_exec(
                                msg.id,
                                &msg.exec_id,
                                &args.url,
                                crate::wire::native_fetch_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::RecordScreen(_) => {
                            let rej = crate::wire::reject_record_screen_exec(
                                msg.id,
                                &msg.exec_id,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::ComputerUse(_) => {
                            let rej = crate::wire::reject_computer_use_exec(
                                msg.id,
                                &msg.exec_id,
                                crate::wire::native_local_exec_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::WriteShellStdin(_) => {
                            let rej = crate::wire::reject_write_shell_stdin_exec(
                                msg.id,
                                &msg.exec_id,
                                crate::wire::native_shell_disabled_message(false),
                            );
                            let agent_bytes = crate::wire::encode_agent_client_exec_message(&rej);
                            let connect_bytes = crate::wire::connect_frame(&agent_bytes);
                            let _ = outbound_tx.send(connect_bytes).await;
                        }
                        ExecServerMessageVariant::Unknown(_, _) => {}
                    }
                }
            }
        }
    }

    let _ = stop_tx.send(());
    let _ = sender.await;
    conn_task.abort();

    if in_thinking {
        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
    }

    if let Some(err) = error_message {
        anyhow::bail!("Cursor agent stream error: {err}");
    }
    if !status.is_success() {
        anyhow::bail!("Cursor agent request failed with HTTP {status}");
    }
    let _ = got_text;

    let _ = tx
        .send(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        }))
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_model_decoder_reads_wire_ids_and_deduplicates() {
        let response = ["composer-2.5", "gpt-5.4-high", "composer-2.5"]
            .into_iter()
            .flat_map(|model| field_ld(1, &field_str(1, model)))
            .collect::<Vec<_>>();
        assert_eq!(
            crate::decode_agent_models(&response).unwrap(),
            vec!["composer-2.5", "gpt-5.4-high"]
        );
    }

    /// Regression test for issue #637: teams routed to a region reject the
    /// hardcoded `global` agent host, so a regional endpoint expressed as a full
    /// URL must reduce to a bare host.
    #[test]
    fn normalize_agent_host_reduces_urls_to_bare_hosts() {
        assert_eq!(
            normalize_agent_host("https://agentn.us.api5.cursor.sh"),
            Some("agentn.us.api5.cursor.sh".to_string())
        );
        assert_eq!(
            normalize_agent_host("http://agentn.eu.api5.cursor.sh/agent.v1.AgentService/Run"),
            Some("agentn.eu.api5.cursor.sh".to_string())
        );
        // Already-bare hosts pass through, case-normalized.
        assert_eq!(
            normalize_agent_host("  AgentN.US.api5.cursor.sh  "),
            Some("agentn.us.api5.cursor.sh".to_string())
        );
        assert_eq!(normalize_agent_host(""), None);
        assert_eq!(normalize_agent_host("   "), None);
        assert_eq!(normalize_agent_host("https://"), None);
    }

    /// The normalized host is used as both the DNS name and the TLS
    /// `ServerName`, so an explicit port must be stripped rather than carried
    /// into either. Raised in review of the #637 fix.
    #[test]
    fn normalize_agent_host_strips_explicit_ports() {
        assert_eq!(
            normalize_agent_host("https://agentn.us.api5.cursor.sh:443/agent.v1.AgentService/Run"),
            Some("agentn.us.api5.cursor.sh".to_string())
        );
        assert_eq!(
            normalize_agent_host("agentn.us.api5.cursor.sh:443"),
            Some("agentn.us.api5.cursor.sh".to_string())
        );
        // A bare port with no host is not a usable host.
        assert_eq!(normalize_agent_host(":443"), None);
    }

    /// `agent_host()` must normalize env overrides the same way it normalizes
    /// the cached CLI value: people copy these out of `cli-config.json` or a
    /// browser, so the full-URL spelling has to work. Raised in review of #637.
    #[test]
    fn agent_host_normalizes_env_overrides() {
        // Serialized against other env-mutating tests in this module by the
        // shared lock in jcode-base.
        let _guard = jcode_base::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_CURSOR_AGENT_HOST");

        jcode_base::env::set_var(
            "JCODE_CURSOR_AGENT_HOST",
            "https://agentn.us.api5.cursor.sh/agent.v1.AgentService/Run",
        );
        assert_eq!(agent_host(), "agentn.us.api5.cursor.sh");

        jcode_base::env::set_var("JCODE_CURSOR_AGENT_HOST", "agentn.eu.api5.cursor.sh:443");
        assert_eq!(agent_host(), "agentn.eu.api5.cursor.sh");

        // A blank override must not win; it falls through to the next source.
        jcode_base::env::set_var("JCODE_CURSOR_AGENT_HOST", "   ");
        assert_ne!(agent_host(), "   ");

        match prev {
            Some(value) => jcode_base::env::set_var("JCODE_CURSOR_AGENT_HOST", value),
            None => jcode_base::env::remove_var("JCODE_CURSOR_AGENT_HOST"),
        }
    }

    #[test]
    fn frames_are_well_formed_connect_frames() {
        let frames = build_run_frames("hi", "composer-2.5", "/tmp", &[], "req");
        assert!(frames.len() >= 4);
        for frame in &frames {
            assert!(frame.len() >= 5);
            let len = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
            assert_eq!(
                len + 5,
                frame.len(),
                "frame length prefix must match payload"
            );
            assert_eq!(frame[0], 0, "request frames are uncompressed data frames");
        }
    }

    #[test]
    fn frame0_contains_prompt_and_model() {
        let frames = build_run_frames("PROMPT_MARKER", "composer-2.5", "/tmp", &[], "req");
        let frame0 = &frames[0];
        let hay = String::from_utf8_lossy(frame0);
        assert!(hay.contains("PROMPT_MARKER"));
        assert!(hay.contains("composer-2.5"));
    }

    #[test]
    fn frame0_advertises_mcp_tools() {
        let tool = jcode_message_types::ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let frames = build_run_frames("hi", "composer-2.5", "/tmp", &[tool], "req");
        let frame0 = &frames[0];
        let hay = String::from_utf8_lossy(frame0);
        assert!(hay.contains("read_file"));
        assert!(hay.contains("Read a file"));
    }

    #[test]
    fn extract_answer_text_reads_nested_chunk() {
        // f1 { f1 { f1: "AUTH" } }
        let leaf = field_str(1, "AUTH");
        let mid = field_ld(1, &leaf);
        let top = field_ld(1, &mid);
        assert_eq!(extract_answer_text(&top).as_deref(), Some("AUTH"));
    }

    #[test]
    fn extract_answer_text_ignores_reasoning() {
        // f1 { f4 { f1: "thinking" } } should not be surfaced as answer text.
        let leaf = field_str(1, "thinking");
        let f4 = field_ld(4, &leaf);
        let top = field_ld(1, &f4);
        assert_eq!(extract_answer_text(&top), None);
        assert_eq!(extract_thinking_text(&top).as_deref(), Some("thinking"));
    }

    #[test]
    fn next_frame_parses_uncompressed() {
        let payload = field_str(1, "hello");
        let frame = connect_frame(&payload);
        let (flag, out, consumed) = next_frame(&frame).unwrap();
        assert_eq!(flag, 0);
        assert_eq!(consumed, frame.len());
        assert_eq!(out, payload);
    }

    #[test]
    fn heartbeat_is_stable() {
        assert_eq!(heartbeat_frame(), vec![0, 0, 0, 0, 2, 0x3a, 0x00]);
    }
}
