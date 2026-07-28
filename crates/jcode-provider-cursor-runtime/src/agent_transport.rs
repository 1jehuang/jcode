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
const CLI_CLIENT_VERSION_DEFAULT: &str = "cli-2026.07.08-0c04a8a";

fn cli_client_version() -> String {
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
fn agent_host() -> String {
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

/// Wrap a protobuf message payload in a Connect data frame (flag 0, uncompressed).
fn connect_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(0);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
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
/// Returns the ordered list of Connect frames that constitute the streamed
/// `RunInput`: `RunRequest`, environment context, then marker frames.
fn build_run_frames(prompt: &str, model: &str, cwd: &str) -> Vec<Vec<u8>> {
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
    req.extend(field_str(4, ""));
    req.extend(field_str(5, &conv));
    req.extend(field_ld(9, &encode_model_meta(model, false)));
    req.extend(field_varint(12, 0));
    // minimal catalog: a "default" entry plus the target model
    req.extend(field_ld(14, &field_str(1, "default")));
    req.extend(field_ld(14, &encode_model_meta(model, false)));
    req.extend(field_str(16, &conv));
    let frame0 = connect_frame(&field_ld(1, &req));

    // frame 1: field 2 = environment context (env block only, no tools/skills)
    let mut env = field_str(1, "linux");
    env.extend(field_str(2, cwd));
    env.extend(field_str(3, "bash"));
    env.extend(field_str(10, "UTC"));
    env.extend(field_str(11, cwd));
    env.extend(field_varint(14, 1));
    env.extend(field_varint(16, 1));
    env.extend(field_varint(19, 0));
    env.extend(field_varint(20, 0));
    env.extend(field_str(21, cwd));
    env.extend(field_varint(22, 0));
    let ctx_payload = field_ld(
        2,
        &field_ld(10, &field_ld(1, &field_ld(1, &field_ld(4, &env)))),
    );
    let frame1 = connect_frame(&ctx_payload);

    // marker frames streamed after the context.
    let mut frames = vec![frame0, frame1];
    frames.push(connect_frame(&field_ld(5, &field_str(1, "")))); // f5{f1:''}
    frames.push(connect_frame(&field_ld(3, &field_str(3, "")))); // f3{f3:''}
    for n in 1..=8u64 {
        // f3{f1:N, f3:''}
        let mut m = field_varint(1, n);
        m.extend(field_str(3, ""));
        frames.push(connect_frame(&field_ld(3, &m)));
    }
    frames
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

/// Run one Cursor agent turn and forward assistant text as [`StreamEvent`]s.
pub async fn run_agent_turn(
    access_token: &str,
    prompt: &str,
    model: &str,
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
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("https://{host}{AGENT_PATH}"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("connect-accept-encoding", "gzip,br")
        .header("connect-protocol-version", "1")
        .header("content-type", "application/connect+proto")
        .header("user-agent", "connect-es/1.6.1")
        .header("x-cursor-client-type", "cli")
        .header("x-cursor-client-version", cli_client_version())
        .header("x-ghost-mode", "true")
        .header("x-request-id", &request_id)
        .header("x-original-request-id", &request_id)
        .body(())
        .context("Failed to build Cursor agent request")?;

    let (response, mut send_stream) = h2
        .send_request(request, false)
        .context("Failed to send Cursor agent request headers")?;

    let session_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, access_token.as_bytes()).to_string();
    let _ = tx.send(Ok(StreamEvent::SessionId(session_id))).await;

    // Sender task: stream the request frames paced like the real client, then
    // heartbeat until the response completes. The pacing is load-bearing: the
    // server treats the marker frames as end-of-input and returns
    // `internal: No exec result` if they arrive before it has begun streaming.
    let frames = build_run_frames(prompt, model, &cwd);
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let sender = tokio::spawn(async move {
        for (idx, frame) in frames.into_iter().enumerate() {
            if send_stream.send_data(Bytes::from(frame), false).is_err() {
                return;
            }
            // frame 0 (RunRequest) and frame 1 (context) need the most settle
            // time before the marker frames follow.
            let pace = match idx {
                0 => Duration::from_millis(1500),
                1 => Duration::from_millis(800),
                _ => Duration::from_millis(400),
            };
            tokio::time::sleep(pace).await;
        }
        let mut ticker = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
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

    // Idle timeouts guard against the server holding the stream open. Cursor
    // keeps the response side open after the assistant message when it expects
    // a tool exec-result (which this text-only transport never sends), so we
    // finish the turn once output goes quiet. The first-byte budget is longer
    // because generation can take a few seconds to start.
    let first_byte_timeout = Duration::from_secs(60);
    let idle_timeout = Duration::from_secs(4);

    'read: loop {
        let budget = if got_text {
            idle_timeout
        } else {
            first_byte_timeout
        };
        let next = match tokio::time::timeout(budget, body.data()).await {
            Ok(Some(chunk)) => chunk,
            // Stream closed cleanly, or idle (server likely waiting for a tool
            // exec-result we never send): finish the turn either way.
            Ok(None) | Err(_) => break 'read,
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
                got_text = true;
                if tx.send(Ok(StreamEvent::TextDelta(text))).await.is_err() {
                    break 'read;
                }
            }
        }
    }

    let _ = stop_tx.send(());
    let _ = sender.await;
    conn_task.abort();

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
        let frames = build_run_frames("hi", "composer-2.5", "/tmp");
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
        let frames = build_run_frames("PROMPT_MARKER", "composer-2.5", "/tmp");
        let frame0 = &frames[0];
        let hay = String::from_utf8_lossy(frame0);
        assert!(hay.contains("PROMPT_MARKER"));
        assert!(hay.contains("composer-2.5"));
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
