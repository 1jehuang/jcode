//! Remote MCP transports: Streamable HTTP (MCP spec 2025-03-26+) and the
//! legacy HTTP+SSE transport (spec 2024-11-05).
//!
//! Both plug into the same [`super::client::McpHandle`] machinery the stdio
//! transport uses: the handle pushes serialized JSON-RPC messages into an mpsc
//! channel and waits on a `pending` map keyed by request id. Here a worker task
//! drains that channel and delivers each message over HTTP, then routes every
//! JSON-RPC response it receives (plain JSON body or SSE stream) back into the
//! `pending` map. Nothing above this module needs to know the transport.
//!
//! Authorization: a static `Authorization` header from the server config wins.
//! Otherwise a stored OAuth token from `jcode mcp login` is attached, and a 401
//! triggers one refresh-and-retry. When no usable credential exists the request
//! fails fast with an actionable "run login" message. Connecting never opens a
//! browser on its own, so background startup can never block on a human.

use super::oauth;
use super::protocol::{JsonRpcError, JsonRpcResponse, McpServerConfig, RemoteTransport};
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};

pub(super) type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

const SESSION_HEADER: &str = "mcp-session-id";
const PROTOCOL_HEADER: &str = "mcp-protocol-version";
/// How long legacy SSE waits for the server's `endpoint` event.
const SSE_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(30);
/// JSON-RPC error code used for transport-level failures we synthesize.
const TRANSPORT_ERROR_CODE: i64 = -32000;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 32;

/// Shared state for one remote connection.
pub(super) struct RemoteState {
    name: String,
    url: reqwest::Url,
    /// Config-provided headers (already env-expanded).
    static_headers: HeaderMap,
    /// True when the config supplies its own `Authorization` header; OAuth is
    /// then never consulted.
    static_auth: bool,
    /// Current OAuth bearer token, if any.
    bearer: std::sync::RwLock<Option<String>>,
    session_id: std::sync::RwLock<Option<String>>,
    protocol_version: std::sync::RwLock<Option<String>>,
    /// Legacy SSE: the POST endpoint announced by the server.
    post_endpoint: std::sync::RwLock<Option<reqwest::Url>>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    stop: Notify,
    request_timeout: Duration,
    /// Set when the server rejected the Streamable HTTP `initialize` POST in a
    /// way that indicates a legacy SSE-only server (400/404/405).
    fallback_to_sse: AtomicBool,
    client: reqwest::Client,
}

impl RemoteState {
    fn auth_header(&self) -> Option<String> {
        if self.static_auth {
            return None;
        }
        self.bearer
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|token| format!("Bearer {token}"))
    }

    fn request(&self, method: reqwest::Method, url: reqwest::Url) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .request(method, url)
            .headers(self.static_headers.clone());
        if let Some(auth) = self.auth_header() {
            builder = builder.header(AUTHORIZATION, auth);
        }
        if let Some(session) = self
            .session_id
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            builder = builder.header(SESSION_HEADER, session);
        }
        if let Some(version) = self
            .protocol_version
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            builder = builder.header(PROTOCOL_HEADER, version);
        }
        builder
    }

    /// After a 401, try to obtain a fresh OAuth token. Returns true when a new
    /// token is available and the request should be retried once.
    async fn recover_from_unauthorized(&self, failed: Option<String>) -> bool {
        if self.static_auth {
            return false;
        }
        match oauth::token_after_unauthorized(self.url.as_str(), failed.as_deref()).await {
            Ok(Some(token)) if Some(&token) != failed.as_ref() => {
                *self.bearer.write().unwrap_or_else(|p| p.into_inner()) = Some(token);
                true
            }
            Ok(_) => false,
            Err(err) => {
                crate::logging::warn(&format!(
                    "MCP [{}]: OAuth token refresh failed: {err:#}",
                    self.name
                ));
                false
            }
        }
    }

    fn unauthorized_message(&self) -> String {
        if self.static_auth {
            format!(
                "MCP server '{}' rejected the configured Authorization header (HTTP 401)",
                self.name
            )
        } else {
            format!(
                "MCP server '{name}' requires authorization. Run `jcode mcp login {name}` in a terminal, then connect again",
                name = self.name
            )
        }
    }

    /// Route one inbound JSON-RPC message (or batch) to its waiter.
    async fn dispatch(&self, value: Value) {
        match value {
            Value::Array(items) => {
                for item in items {
                    Box::pin(self.dispatch(item)).await;
                }
            }
            Value::Object(ref map) => {
                let is_response = map.contains_key("result") || map.contains_key("error");
                if !is_response {
                    // Server-initiated request or notification. We advertise no
                    // client capabilities, so there is nothing to act on.
                    crate::logging::debug(&format!(
                        "MCP [{}]: ignoring server message {}",
                        self.name,
                        map.get("method").and_then(Value::as_str).unwrap_or("?")
                    ));
                    return;
                }
                // Notifications and requests cannot impersonate response envelopes.
                if map.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
                    || map.contains_key("method")
                    || map.contains_key("result") == map.contains_key("error")
                {
                    return;
                }
                match serde_json::from_value::<JsonRpcResponse>(value) {
                    Ok(response) => {
                        if let Some(id) = response.id
                            && let Some(tx) = self.pending.lock().await.remove(&id)
                        {
                            let _ = tx.send(response);
                        }
                    }
                    Err(err) => crate::logging::debug(&format!(
                        "MCP [{}]: unparseable response: {err}",
                        self.name
                    )),
                }
            }
            _ => {}
        }
    }

    /// Fail an in-flight request with a transport error so its caller returns
    /// immediately instead of waiting out the request timeout.
    async fn fail(&self, id: Option<u64>, message: String) {
        crate::logging::warn(&format!("MCP [{}]: {message}", self.name));
        let Some(id) = id else { return };
        if let Some(tx) = self.pending.lock().await.remove(&id) {
            let _ = tx.send(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(id),
                result: None,
                error: Some(JsonRpcError {
                    code: TRANSPORT_ERROR_CODE,
                    message,
                    data: None,
                }),
            });
        }
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.stop.notify_one();
    }

    async fn close(&self, message: &str) {
        self.mark_closed();
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (id, tx) in pending {
            let _ = tx.send(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(id),
                result: None,
                error: Some(JsonRpcError {
                    code: TRANSPORT_ERROR_CODE,
                    message: message.to_string(),
                    data: None,
                }),
            });
        }
    }

    fn bearer_token(&self) -> Option<String> {
        self.bearer
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// Metadata about an outbound message needed for routing errors.
struct Outbound {
    id: Option<u64>,
    method: Option<String>,
}

fn outbound_info(body: &str) -> Outbound {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    Outbound {
        id: value.get("id").and_then(Value::as_u64),
        method: value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

async fn read_body(
    response: reqwest::Response,
    limit: usize,
) -> std::result::Result<String, String> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(format!("MCP HTTP response exceeds {limit} byte limit"));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| format!("MCP response read failed: {}", err.without_url()))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(format!("MCP HTTP response exceeds {limit} byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| "MCP HTTP response is not UTF-8".to_string())
}

fn snippet(text: &str) -> String {
    let trimmed = text.trim();
    let mut out: String = trimmed.chars().take(300).collect();
    if trimmed.chars().count() > 300 {
        out.push('…');
    }
    out
}

/// A live remote connection. Dropping it stops its background tasks.
pub(super) struct RemoteConnection {
    state: Arc<RemoteState>,
    tasks: Vec<JoinHandle<()>>,
}

impl RemoteConnection {
    pub(super) fn is_running(&self) -> bool {
        !self.state.closed.load(Ordering::SeqCst)
    }

    pub(super) fn is_legacy(&self) -> bool {
        self.state
            .post_endpoint
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    pub(super) fn wants_sse_fallback(&self) -> bool {
        self.state.fallback_to_sse.load(Ordering::SeqCst)
    }

    pub(super) fn set_protocol_version(&self, version: &str) {
        *self
            .state
            .protocol_version
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(version.to_string());
    }

    /// Terminate the session (best effort) and stop background tasks.
    pub(super) async fn shutdown(&mut self) {
        // Stop and reap workers first. Their JoinSets own every HTTP request,
        // including requests blocked while receiving an unbounded SSE stream.
        self.state.close("MCP connection shut down").await;
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
        let session = self
            .state
            .session_id
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if session.is_some() {
            let request = self
                .state
                .request(reqwest::Method::DELETE, self.state.url.clone());
            let _ = tokio::time::timeout(Duration::from_secs(2), request.send()).await;
        }
    }
}

impl Drop for RemoteConnection {
    fn drop(&mut self) {
        self.state.mark_closed();
        for task in &self.tasks {
            task.abort();
        }
        // Handles may outlive their owner. Wake them instead of waiting for
        // their configured timeout after the connection has been dropped.
        if let Ok(mut pending) = self.state.pending.try_lock() {
            pending.clear();
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let pending = Arc::clone(&self.state.pending);
            runtime.spawn(async move { pending.lock().await.clear() });
        }
    }
}

fn build_static_headers(config: &McpServerConfig) -> Result<(HeaderMap, bool)> {
    let mut headers = HeaderMap::new();
    let mut has_auth = false;
    for (key, value) in &config.headers {
        let name = HeaderName::from_bytes(key.trim().as_bytes())
            .with_context(|| format!("invalid header name '{key}'"))?;
        if matches!(
            name.as_str(),
            "host" | "content-length" | "transfer-encoding" | SESSION_HEADER | PROTOCOL_HEADER
        ) {
            bail!("header '{key}' is controlled by the MCP transport");
        }
        if name == AUTHORIZATION {
            has_auth = true;
        }
        let value = HeaderValue::from_str(value.trim())
            .with_context(|| format!("invalid value for header '{key}'"))?;
        headers.insert(name, value);
    }
    Ok((headers, has_auth))
}

/// Start a remote transport. The caller still has to run the MCP
/// `initialize` handshake over the returned handle channels.
pub(super) async fn start(
    name: &str,
    config: &McpServerConfig,
    transport: RemoteTransport,
    writer_rx: mpsc::Receiver<String>,
    pending: Pending,
    closed: Arc<AtomicBool>,
) -> Result<RemoteConnection> {
    let raw_url = config
        .url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .with_context(|| format!("MCP server '{name}' has no url"))?;
    let url =
        reqwest::Url::parse(raw_url).with_context(|| format!("invalid MCP url '{raw_url}'"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("MCP url must be http(s): {raw_url}");
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        bail!("MCP URL must not contain userinfo or a fragment");
    }
    let (static_headers, static_auth) = build_static_headers(config)?;
    let bearer = if static_auth {
        None
    } else {
        oauth::access_token(url.as_str())
            .await
            .unwrap_or_else(|err| {
                crate::logging::warn(&format!(
                    "MCP [{name}]: could not load stored OAuth token: {err:#}"
                ));
                None
            })
    };

    let state = Arc::new(RemoteState {
        name: name.to_string(),
        url,
        static_headers,
        static_auth,
        bearer: std::sync::RwLock::new(bearer),
        session_id: std::sync::RwLock::new(None),
        protocol_version: std::sync::RwLock::new(None),
        post_endpoint: std::sync::RwLock::new(None),
        pending,
        closed,
        stop: Notify::new(),
        request_timeout: super::client::request_timeout_for(config),
        fallback_to_sse: AtomicBool::new(false),
        // Even custom headers (not just Authorization) can contain credentials.
        // Never follow a redirect with them to a server-selected destination.
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("build MCP HTTP client")?,
    });

    // Construct the RAII owner before any fallible setup, so a timeout or
    // cancellation while waiting for a legacy endpoint cannot leak its reader.
    let mut connection = RemoteConnection {
        state: Arc::clone(&state),
        tasks: Vec::new(),
    };
    match transport {
        RemoteTransport::StreamableHttp => {
            connection.tasks.push(tokio::spawn(outbound_worker(
                Arc::clone(&state),
                writer_rx,
                false,
            )));
        }
        RemoteTransport::LegacySse => {
            let (endpoint_tx, endpoint_rx) = oneshot::channel();
            let response = tokio::time::timeout(state.request_timeout, open_sse_stream(&state))
                .await
                .context("SSE connection timed out")??;
            connection.tasks.push(tokio::spawn(sse_reader(
                Arc::clone(&state),
                response,
                endpoint_tx,
            )));
            let endpoint =
                tokio::time::timeout(SSE_ENDPOINT_TIMEOUT.min(state.request_timeout), endpoint_rx)
                    .await
                    .with_context(|| {
                        format!("MCP server '{name}' did not announce an SSE endpoint")
                    })?
                    .with_context(|| format!("MCP server '{name}' closed the SSE stream"))??;
            *state
                .post_endpoint
                .write()
                .unwrap_or_else(|p| p.into_inner()) = Some(endpoint);
            connection.tasks.push(tokio::spawn(outbound_worker(
                Arc::clone(&state),
                writer_rx,
                true,
            )));
        }
    }

    Ok(connection)
}

// ---------------------------------------------------------------------------
// Streamable HTTP
// ---------------------------------------------------------------------------

async fn outbound_worker(
    state: Arc<RemoteState>,
    mut writer_rx: mpsc::Receiver<String>,
    legacy: bool,
) {
    let mut requests = JoinSet::new();
    loop {
        if state.closed.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            biased;
            _ = state.stop.notified() => break,
            _ = requests.join_next(), if !requests.is_empty() => {},
            body = writer_rx.recv(), if requests.len() < MAX_CONCURRENT_REQUESTS => {
                let Some(body) = body else { break };
                let info = outbound_info(&body);
                if info.id.is_none() {
                    // An initialized notification must finish posting before
                    // tools/list starts, but cannot block shutdown.
                    tokio::select! {
                        _ = state.stop.notified() => break,
                        _ = post_message(Arc::clone(&state), body, info, legacy) => {},
                    }
                } else {
                    requests.spawn(post_message(Arc::clone(&state), body, info, legacy));
                }
            }
        }
    }
    requests.shutdown().await;
    state.close("MCP transport closed").await;
}

async fn post_message(state: Arc<RemoteState>, body: String, info: Outbound, legacy: bool) {
    // Skip queued requests whose caller has already timed out or cancelled.
    if let Some(id) = info.id
        && !state.pending.lock().await.contains_key(&id)
    {
        return;
    }
    let post = async {
        if legacy {
            sse_post_inner(&state, &body, &info).await
        } else {
            streamable_post_inner(&state, &body, &info).await
        }
    };
    let cancelled = async {
        let Some(id) = info.id else {
            std::future::pending::<()>().await;
            return;
        };
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if !state.pending.lock().await.contains_key(&id) {
                break;
            }
        }
    };
    let result = tokio::select! {
        _ = cancelled => return,
        result = tokio::time::timeout(state.request_timeout, post) =>
            result.unwrap_or_else(|_| Err("MCP HTTP request timed out".to_string())),
    };
    if let Err(message) = result {
        if info.id.is_none() {
            state.close(&message).await;
        } else {
            state.fail(info.id, message).await;
        }
    }
}

async fn send_post(
    state: &RemoteState,
    url: reqwest::Url,
    body: &str,
) -> reqwest::Result<reqwest::Response> {
    state
        .request(reqwest::Method::POST, url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(body.to_string())
        .send()
        .await
}

async fn streamable_post_inner(
    state: &Arc<RemoteState>,
    body: &str,
    info: &Outbound,
) -> std::result::Result<(), String> {
    let is_initialize = info.method.as_deref() == Some("initialize");
    let send = |state: &Arc<RemoteState>| {
        let url = state.url.clone();
        let body = body.to_string();
        let state = Arc::clone(state);
        async move { send_post(&state, url, &body).await }
    };

    let failed = state.bearer_token();
    let mut response = send(state).await.map_err(|err| {
        format!(
            "HTTP request to MCP server '{}' failed: {}",
            state.name,
            err.without_url()
        )
    })?;
    if response.status() == StatusCode::UNAUTHORIZED
        && state.recover_from_unauthorized(failed).await
    {
        response = send(state).await.map_err(|err| {
            format!(
                "HTTP request to MCP server '{}' failed: {}",
                state.name,
                err.without_url()
            )
        })?;
    }

    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err(state.unauthorized_message());
    }
    if status == StatusCode::NOT_FOUND
        && state
            .session_id
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    {
        // The server dropped our session. Mark the connection dead so the next
        // tool call reconnects with a fresh `initialize`.
        let message = format!(
            "MCP server '{}' session expired; reconnect to start a new session",
            state.name
        );
        state.close(&message).await;
        return Err(message);
    }
    if !status.is_success() {
        if is_initialize
            && matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            )
        {
            state.fallback_to_sse.store(true, Ordering::SeqCst);
        }
        let text = read_body(response, MAX_ERROR_BYTES)
            .await
            .unwrap_or_default();
        return Err(format!(
            "MCP server '{}' returned HTTP {}: {}",
            state.name,
            status,
            snippet(&text)
        ));
    }

    if is_initialize
        && let Some(session) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
    {
        if session.is_empty() || !session.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return Err("MCP server returned an invalid session ID".to_string());
        }
        *state.session_id.write().unwrap_or_else(|p| p.into_inner()) = Some(session.to_string());
    }

    if info.id.is_none() {
        return Ok(());
    }
    if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
        return Err("MCP server accepted a request without a JSON-RPC response".to_string());
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.starts_with("text/event-stream") {
        let mut stream = response.bytes_stream();
        let mut parser = SseParser::default();
        let mut received = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| {
                format!("MCP server '{}' response stream failed: {err}", state.name)
            })?;
            received = received.saturating_add(chunk.len());
            if received > MAX_RESPONSE_BYTES {
                return Err("MCP SSE response exceeds size limit".to_string());
            }
            for event in parser.push(&chunk).map_err(|err| err.to_string())? {
                if event
                    .event
                    .as_deref()
                    .is_none_or(|kind| kind.is_empty() || kind == "message")
                {
                    let value = serde_json::from_str::<Value>(&event.data)
                        .map_err(|err| format!("MCP SSE returned invalid JSON: {err}"))?;
                    state.dispatch(value).await;
                }
            }
            if let Some(id) = info.id
                && !state.pending.lock().await.contains_key(&id)
            {
                // Our response arrived; stop reading so the server can close.
                return Ok(());
            }
        }
        if let Some(id) = info.id
            && state.pending.lock().await.contains_key(&id)
        {
            return Err(format!(
                "MCP server '{}' closed the response stream without answering",
                state.name
            ));
        }
        return Ok(());
    }

    if content_type.split(';').next().map(str::trim) != Some("application/json") {
        return Err(format!(
            "MCP server returned unsupported Content-Type: {content_type}"
        ));
    }
    let text = read_body(response, MAX_RESPONSE_BYTES).await?;
    if text.trim().is_empty() {
        return Err(format!(
            "MCP server '{}' returned an empty response",
            state.name
        ));
    }
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        format!(
            "MCP server '{}' returned invalid JSON ({err}): {}",
            state.name,
            snippet(&text)
        )
    })?;
    state.dispatch(value).await;
    if let Some(id) = info.id
        && state.pending.lock().await.contains_key(&id)
    {
        return Err("MCP server returned no matching JSON-RPC response".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy HTTP+SSE
// ---------------------------------------------------------------------------

async fn open_sse_stream(state: &Arc<RemoteState>) -> Result<reqwest::Response> {
    let get = |state: &Arc<RemoteState>| {
        state
            .request(reqwest::Method::GET, state.url.clone())
            .header(ACCEPT, "text/event-stream")
            .send()
    };
    let failed = state.bearer_token();
    let mut response = get(state)
        .await
        .map_err(reqwest::Error::without_url)
        .context("SSE connect failed")?;
    if response.status() == StatusCode::UNAUTHORIZED
        && state.recover_from_unauthorized(failed).await
    {
        response = get(state)
            .await
            .map_err(reqwest::Error::without_url)
            .context("SSE connect failed")?;
    }
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        bail!(state.unauthorized_message());
    }
    if !status.is_success() {
        let text = read_body(response, MAX_ERROR_BYTES)
            .await
            .unwrap_or_default();
        bail!("SSE connect returned HTTP {status}: {}", snippet(&text));
    }
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| {
            !v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
        })
    {
        bail!("MCP legacy GET did not return text/event-stream");
    }
    Ok(response)
}

fn validate_endpoint(base: &reqwest::Url, data: &str) -> Result<reqwest::Url> {
    let url = base.join(data.trim()).context("invalid SSE endpoint URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!(
            "MCP SSE endpoint must have the same origin as the configured URL and no userinfo or fragment"
        );
    }
    Ok(url)
}

async fn sse_reader(
    state: Arc<RemoteState>,
    response: reqwest::Response,
    endpoint_tx: oneshot::Sender<Result<reqwest::Url>>,
) {
    let mut endpoint_tx = Some(endpoint_tx);
    let mut stream = response.bytes_stream();
    let mut parser = SseParser::default();
    let result: Result<()> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(reqwest::Error::without_url)?;
            for event in parser.push(&chunk)? {
                if event.event.as_deref() == Some("endpoint") {
                    if let Some(tx) = endpoint_tx.take() {
                        let endpoint = validate_endpoint(&state.url, &event.data);
                        let valid = endpoint.is_ok();
                        let _ = tx.send(endpoint);
                        if !valid {
                            bail!("invalid MCP SSE endpoint");
                        }
                    }
                    continue;
                }
                if endpoint_tx.is_some() {
                    // The legacy transport requires its endpoint announcement
                    // before any JSON-RPC messages.
                    bail!("MCP SSE first event was not an endpoint");
                }
                if event
                    .event
                    .as_deref()
                    .is_none_or(|kind| kind.is_empty() || kind == "message")
                {
                    let value = serde_json::from_str::<Value>(&event.data)
                        .context("MCP SSE message contains invalid JSON")?;
                    state.dispatch(value).await;
                }
            }
        }
        Ok(())
    }
    .await;
    if let Some(tx) = endpoint_tx {
        let _ = tx.send(Err(anyhow::anyhow!(
            "MCP SSE stream ended before announcing an endpoint"
        )));
    }
    let message = match result {
        Ok(()) => "MCP SSE stream closed".to_string(),
        Err(err) => format!("MCP SSE stream failed: {err:#}"),
    };
    state.close(&message).await;
}

async fn sse_post_inner(
    state: &Arc<RemoteState>,
    body: &str,
    _info: &Outbound,
) -> std::result::Result<(), String> {
    if state.closed.load(Ordering::SeqCst) {
        return Err("MCP SSE connection is closed".to_string());
    }
    let endpoint = state
        .post_endpoint
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .ok_or_else(|| "MCP SSE connection has no endpoint".to_string())?;
    let failed = state.bearer_token();
    let mut response = send_post(state, endpoint.clone(), body)
        .await
        .map_err(|err| format!("MCP HTTP request failed: {}", err.without_url()))?;
    if response.status() == StatusCode::UNAUTHORIZED
        && state.recover_from_unauthorized(failed).await
    {
        response = send_post(state, endpoint, body)
            .await
            .map_err(|err| format!("MCP HTTP request failed: {}", err.without_url()))?;
    }
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(state.unauthorized_message());
    }
    if !response.status().is_success() {
        let status = response.status();
        let text = read_body(response, MAX_ERROR_BYTES)
            .await
            .unwrap_or_default();
        return Err(format!(
            "MCP server '{}' returned HTTP {status}: {}",
            state.name,
            snippet(&text)
        ));
    }
    Ok(()) // JSON-RPC replies arrive exclusively on the legacy SSE stream.
}

// ---------------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental UTF-8 SSE parser supporting LF, CRLF and bare CR, a leading
/// BOM, comments, empty fields and multiline data. Bounds apply to each event
/// and line, not the lifetime of an indefinitely running legacy stream.
#[derive(Default)]
pub(super) struct SseParser {
    line: Vec<u8>,
    event: Option<String>,
    data: String,
    has_data: bool,
    event_bytes: usize,
    skip_lf: bool,
    first_line: bool,
}

impl SseParser {
    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>> {
        if chunk.len() > MAX_RESPONSE_BYTES {
            bail!("MCP SSE chunk exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        let mut events = Vec::new();
        for &byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\n' || byte == b'\r' {
                self.skip_lf = byte == b'\r';
                if let Some(event) = self.finish_line()? {
                    if events.len() >= 4096 {
                        bail!("too many MCP SSE events in one chunk");
                    }
                    events.push(event);
                }
            } else {
                if self.line.len() >= MAX_RESPONSE_BYTES {
                    bail!("MCP SSE line exceeds {MAX_RESPONSE_BYTES} bytes");
                }
                self.line.push(byte);
            }
        }
        Ok(events)
    }

    fn finish_line(&mut self) -> Result<Option<SseEvent>> {
        let bytes = std::mem::take(&mut self.line);
        let mut line = String::from_utf8(bytes).context("MCP SSE is not UTF-8")?;
        if !self.first_line {
            self.first_line = true;
            if let Some(rest) = line.strip_prefix('\u{feff}') {
                line = rest.to_string();
            }
        }
        if line.is_empty() {
            self.event_bytes = 0;
            let event = self.event.take();
            if self.has_data {
                self.has_data = false;
                self.data.pop(); // Remove only the last data-line newline.
                return Ok(Some(SseEvent {
                    event,
                    data: std::mem::take(&mut self.data),
                }));
            }
            return Ok(None);
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        if matches!(field, "event" | "data") {
            self.event_bytes = self.event_bytes.saturating_add(value.len() + 1);
            if self.event_bytes > MAX_RESPONSE_BYTES {
                bail!("MCP SSE event exceeds {MAX_RESPONSE_BYTES} bytes");
            }
        }
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => {
                self.has_data = true;
                self.data.push_str(value);
                self.data.push('\n');
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod remote_tests;
