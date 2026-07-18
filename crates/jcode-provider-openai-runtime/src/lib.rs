//! OpenAI provider runtime (Codex OAuth + API key, Responses API over SSE and
//! persistent WebSocket), moved out of `jcode-base` so provider edits compile
//! only this crate plus a binary relink instead of rebuilding the
//! base -> app-core -> tui spine. The binary's composition root registers
//! [`OpenAIProvider`] with `jcode_base::provider::external` at startup.
//!
//! Model-catalog/account-availability state stays in `jcode_base::provider`
//! (it is shared vocabulary for routing), as does the pure request shaping in
//! `jcode_base::provider::openai_request`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{FutureExt, SinkExt, StreamExt as FuturesStreamExt};
use jcode_base::auth::codex::CodexCredentials;
use jcode_base::auth::oauth;
use jcode_base::provider::openai_request::{build_responses_input, build_tools};
#[cfg(test)]
use jcode_message_types::TOOL_OUTPUT_MISSING_TEXT;
use jcode_message_types::{Message as ChatMessage, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, Provider};

#[cfg(test)]
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
use reqwest::header::HeaderValue;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock, RwLock as StdRwLock};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const RESPONSES_PATH: &str = "responses";
const DEFAULT_MODEL: &str = jcode_provider_core::DEFAULT_OPENAI_MODEL;
const ORIGINATOR: &str = "codex_cli_rs";

pub(crate) const CHATGPT_WEB_MODEL: &str = jcode_provider_core::CHATGPT_WEB_MODEL;

pub(crate) fn is_chatgpt_web_model(model: &str) -> bool {
    model.trim() == CHATGPT_WEB_MODEL
}

/// Whether the hosted `image_generation` tool can be attached for `model_id`.
///
/// The Responses backend only exposes `image_generation` to general
/// ChatGPT/GPT models. Codex models (ids containing `codex`) reject unknown
/// hosted tools, so they must not receive it. See issue #369.
fn model_supports_image_generation(model_id: &str) -> bool {
    !model_id.to_ascii_lowercase().contains("codex")
}

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;
const WEBSOCKET_UPGRADE_REQUIRED_ERROR: StatusCode = StatusCode::UPGRADE_REQUIRED;
const WEBSOCKET_CONNECT_TIMEOUT_SECS: u64 = 8;
/// Maximum age of a persistent WebSocket connection before forcing reconnect
const WEBSOCKET_PERSISTENT_MAX_AGE_SECS: u64 = 3000; // 50 min (server limit is 60 min)
/// Default idle window after which we reconnect instead of reusing the socket.
///
/// Raised from the original 90s. Tearing the socket down loses the server-side
/// `previous_response_id` chain, so the next turn must re-send the full
/// conversation and relies on OpenAI prefix-hash routing, which frequently
/// lands on a cold machine (observed zero cache reads). The lightweight
/// healthcheck ping below (1.5s timeout) is the real liveness probe, so for
/// typical interactive pauses we prefer to confirm-and-reuse the live socket
/// and keep the warm cache. Dead/half-closed sockets are still detected by the
/// ping and reconnect gracefully, and `WEBSOCKET_PERSISTENT_MAX_AGE_SECS` still
/// caps total connection lifetime. Tunable via
/// `JCODE_OPENAI_WS_IDLE_RECONNECT_SECS` (0 disables the idle reconnect entirely,
/// relying solely on the healthcheck + max-age cap).
const WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS_DEFAULT: u64 = 600; // 10 min
/// If a persistent socket has been idle for a while, send a lightweight ping
/// before reuse so we can proactively detect half-closed connections.
const WEBSOCKET_PERSISTENT_HEALTHCHECK_IDLE_SECS: u64 = 15;

/// Resolved idle-reconnect threshold (seconds), read once from the environment.
/// `Some(secs)` means reconnect after that idle duration; `None` means never
/// force a reconnect on idle alone (healthcheck + max-age still apply).
static WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS: LazyLock<Option<u64>> = LazyLock::new(|| {
    match std::env::var("JCODE_OPENAI_WS_IDLE_RECONNECT_SECS") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(0) => {
                jcode_base::logging::info(
                    "OpenAI persistent WS idle reconnect disabled (JCODE_OPENAI_WS_IDLE_RECONNECT_SECS=0); relying on healthcheck + max-age",
                );
                None
            }
            Ok(secs) => {
                jcode_base::logging::info(&format!(
                    "OpenAI persistent WS idle reconnect threshold set to {}s (JCODE_OPENAI_WS_IDLE_RECONNECT_SECS)",
                    secs
                ));
                Some(secs)
            }
            Err(_) => {
                jcode_base::logging::info(&format!(
                    "Warning: invalid JCODE_OPENAI_WS_IDLE_RECONNECT_SECS '{}'; using default {}s",
                    raw, WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS_DEFAULT
                ));
                Some(WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS_DEFAULT)
            }
        },
        Err(_) => Some(WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS_DEFAULT),
    }
});
const WEBSOCKET_PERSISTENT_HEALTHCHECK_TIMEOUT_MS: u64 = 1500;
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 32_768;
static WEBSOCKET_COOLDOWNS: LazyLock<Arc<RwLock<HashMap<String, Instant>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));
static WEBSOCKET_FAILURE_STREAKS: LazyLock<Arc<RwLock<HashMap<String, u32>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));

#[expect(
    clippy::upper_case_acronyms,
    reason = "transport names mirror user-facing configuration values like https and websocket"
)]
#[derive(Clone, Copy)]
enum OpenAITransportMode {
    Auto,
    WebSocket,
    HTTPS,
}

impl OpenAITransportMode {
    fn from_config(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::Auto;
        };
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Self::Auto,
            "websocket" | "ws" | "wss" => Self::WebSocket,
            "https" | "http" | "sse" => Self::HTTPS,
            other => {
                jcode_base::logging::warn(&format!(
                    "Unknown JCODE_OPENAI_TRANSPORT '{}'; using auto. Use: auto, websocket, or https.",
                    other
                ));
                Self::Auto
            }
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::WebSocket => "websocket",
            Self::HTTPS => "https",
        }
    }
}

#[derive(Debug)]
enum OpenAIStreamFailure {
    FallbackToHttps(anyhow::Error),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for OpenAIStreamFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

#[expect(
    clippy::upper_case_acronyms,
    reason = "transport names mirror user-facing configuration values like https and websocket"
)]
#[derive(Clone, Copy)]
enum OpenAITransport {
    WebSocket,
    HTTPS,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAINativeCompactionMode {
    Auto,
    Explicit,
    Off,
}

/// Shared dual-auth credential pin (see `jcode_provider_core::CredentialMode`).
/// The OpenAI-specific alias is kept so existing call sites read naturally.
pub(crate) use jcode_provider_core::CredentialMode as OpenAICredentialMode;

/// Load Codex credentials for the given credential pin.
pub(crate) fn load_credentials_for_mode(mode: OpenAICredentialMode) -> Result<CodexCredentials> {
    match mode {
        OpenAICredentialMode::Auto => jcode_base::auth::codex::load_credentials(),
        OpenAICredentialMode::OAuth => jcode_base::auth::codex::load_oauth_credentials(),
        OpenAICredentialMode::ApiKey => jcode_base::auth::codex::load_api_key_credentials(),
    }
}

impl OpenAINativeCompactionMode {
    fn from_config(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Self::Auto,
            "explicit" | "manual" => Self::Explicit,
            "off" | "disabled" | "none" => Self::Off,
            other => {
                jcode_base::logging::warn(&format!(
                    "Unknown OpenAI native compaction mode '{}'; using auto. Use: auto, explicit, or off.",
                    other
                ));
                Self::Auto
            }
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Explicit => "explicit",
            Self::Off => "off",
        }
    }
}

impl OpenAITransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::WebSocket => "websocket",
            Self::HTTPS => "https",
        }
    }
}

fn log_openai_stream_lifecycle(
    level: jcode_base::logging::LogLevel,
    phase: &str,
    fields: Vec<(&str, String)>,
) {
    let mut owned = vec![
        ("phase".to_string(), phase.to_string()),
        ("provider".to_string(), "openai".to_string()),
    ];
    owned.extend(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    );
    jcode_base::logging::event(level, "PROVIDER_STREAM_LIFECYCLE", owned);
}

fn openai_request_model(request: &Value) -> String {
    request
        .get("model")
        .and_then(|model| model.as_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Persistent WebSocket connection state for incremental continuation.
/// Keeps the connection alive across turns so we can use `previous_response_id`
/// to send only new items instead of the full conversation each turn.
struct PersistentWsState {
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    last_response_id: String,
    connected_at: Instant,
    last_activity_at: Instant,
    /// Number of messages sent in this conversation chain
    message_count: usize,
    /// Number of items we sent in the last full request (for detecting conversation changes)
    last_input_item_count: usize,
}

#[derive(Debug, Clone)]
struct PersistentWsDiagSnapshot {
    present: bool,
    connected_age_ms: Option<u128>,
    idle_age_ms: Option<u128>,
    message_count: Option<usize>,
    last_input_item_count: Option<usize>,
    previous_response_id_present: Option<bool>,
}

impl PersistentWsDiagSnapshot {
    fn absent() -> Self {
        Self {
            present: false,
            connected_age_ms: None,
            idle_age_ms: None,
            message_count: None,
            last_input_item_count: None,
            previous_response_id_present: None,
        }
    }

    fn log_fields(&self) -> String {
        if !self.present {
            return "persistent_ws=absent".to_string();
        }

        format!(
            "persistent_ws=present connected_age_ms={} idle_age_ms={} message_count={} last_input_items={} previous_response_id_present={}",
            self.connected_age_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            self.idle_age_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            self.message_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            self.last_input_item_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            self.previous_response_id_present
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        )
    }
}

impl PersistentWsState {
    fn diag_snapshot(&self) -> PersistentWsDiagSnapshot {
        PersistentWsDiagSnapshot {
            present: true,
            connected_age_ms: Some(self.connected_at.elapsed().as_millis()),
            idle_age_ms: Some(self.last_activity_at.elapsed().as_millis()),
            message_count: Some(self.message_count),
            last_input_item_count: Some(self.last_input_item_count),
            previous_response_id_present: Some(!self.last_response_id.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WsInputStats {
    total_items: usize,
    message_items: usize,
    function_call_items: usize,
    function_call_output_items: usize,
    other_items: usize,
}

impl WsInputStats {
    fn tool_callback_count(self) -> usize {
        self.function_call_output_items
    }

    fn log_fields(self) -> String {
        format!(
            "items={} messages={} function_calls={} tool_outputs={} other={}",
            self.total_items,
            self.message_items,
            self.function_call_items,
            self.function_call_output_items,
            self.other_items
        )
    }
}

fn summarize_ws_input(items: &[Value]) -> WsInputStats {
    let mut stats = WsInputStats::default();
    for item in items {
        stats.total_items += 1;
        match item.get("type").and_then(|value| value.as_str()) {
            Some("message") => stats.message_items += 1,
            Some("function_call") => stats.function_call_items += 1,
            Some("function_call_output") => stats.function_call_output_items += 1,
            _ => stats.other_items += 1,
        }
    }
    stats
}

fn persistent_ws_incremental_items(input: &[Value], start_index: usize) -> (Vec<Value>, usize) {
    let mut skipped_reasoning_items = 0usize;
    let incremental_items = input[start_index..]
        .iter()
        .filter_map(|item| {
            if item.get("type").and_then(|value| value.as_str()) == Some("reasoning") {
                skipped_reasoning_items += 1;
                None
            } else {
                Some(item.clone())
            }
        })
        .collect();
    (incremental_items, skipped_reasoning_items)
}

fn persistent_ws_idle_needs_healthcheck(idle_for: Duration) -> bool {
    idle_for >= Duration::from_secs(WEBSOCKET_PERSISTENT_HEALTHCHECK_IDLE_SECS)
}

fn idle_requires_reconnect_with(threshold_secs: Option<u64>, idle_for: Duration) -> bool {
    match threshold_secs {
        Some(secs) => idle_for >= Duration::from_secs(secs),
        None => false,
    }
}

fn persistent_ws_idle_requires_reconnect(idle_for: Duration) -> bool {
    idle_requires_reconnect_with(*WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS, idle_for)
}

async fn emit_connection_phase(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    phase: jcode_message_types::ConnectionPhase,
) {
    let _ = tx.send(Ok(StreamEvent::ConnectionPhase { phase })).await;
}

async fn emit_status_detail(tx: &mpsc::Sender<Result<StreamEvent>>, detail: impl Into<String>) {
    let _ = tx
        .send(Ok(StreamEvent::StatusDetail {
            detail: detail.into(),
        }))
        .await;
}

fn format_status_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {}m", hours, mins)
    } else if secs >= 60 {
        let mins = secs / 60;
        let rem_secs = secs % 60;
        format!("{}m {}s", mins, rem_secs)
    } else {
        format!("{}s", secs)
    }
}

async fn ensure_persistent_ws_is_healthy(state: &mut PersistentWsState) -> Result<bool, String> {
    let idle_for = state.last_activity_at.elapsed();
    if persistent_ws_idle_requires_reconnect(idle_for) {
        jcode_base::logging::info(&format!(
            "Persistent WS idle for {}s; reconnecting before reuse",
            idle_for.as_secs()
        ));
        return Ok(false);
    }

    if !persistent_ws_idle_needs_healthcheck(idle_for) {
        return Ok(true);
    }

    jcode_base::logging::info(&format!(
        "Persistent WS idle for {}ms; sending healthcheck ping before reuse",
        idle_for.as_millis()
    ));

    state
        .ws_stream
        .send(WsMessage::Ping(Vec::new()))
        .await
        .map_err(|err| format!("healthcheck ping send error: {}", err))?;

    let started_at = Instant::now();
    let timeout = Duration::from_millis(WEBSOCKET_PERSISTENT_HEALTHCHECK_TIMEOUT_MS);

    while started_at.elapsed() < timeout {
        let remaining = timeout.saturating_sub(started_at.elapsed());
        let next_item = tokio::time::timeout(remaining, state.ws_stream.next())
            .await
            .map_err(|_| {
                format!(
                    "healthcheck pong timeout after {}ms",
                    WEBSOCKET_PERSISTENT_HEALTHCHECK_TIMEOUT_MS
                )
            })?;

        match next_item {
            Some(Ok(WsMessage::Pong(_))) => {
                state.last_activity_at = Instant::now();
                jcode_base::logging::info(&format!(
                    "Persistent WS healthcheck pong after {}ms",
                    started_at.elapsed().as_millis()
                ));
                return Ok(true);
            }
            Some(Ok(WsMessage::Ping(payload))) => {
                state
                    .ws_stream
                    .send(WsMessage::Pong(payload))
                    .await
                    .map_err(|err| format!("healthcheck pong send error: {}", err))?;
                state.last_activity_at = Instant::now();
            }
            Some(Ok(WsMessage::Close(_))) => {
                return Ok(false);
            }
            Some(Ok(other)) => {
                return Err(format!(
                    "unexpected websocket frame during healthcheck: {:?}",
                    other
                ));
            }
            Some(Err(err)) => {
                return Err(format!("healthcheck receive error: {}", err));
            }
            None => {
                return Ok(false);
            }
        }
    }

    Ok(false)
}

pub struct OpenAIProvider {
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    credential_mode: Arc<RwLock<OpenAICredentialMode>>,
    model: Arc<RwLock<String>>,
    prompt_cache_key: Option<String>,
    prompt_cache_retention: Option<String>,
    max_output_tokens: Option<u32>,
    reasoning_effort: Arc<StdRwLock<Option<String>>>,
    service_tier: Arc<StdRwLock<Option<String>>>,
    native_compaction_mode: OpenAINativeCompactionMode,
    native_compaction_threshold_tokens: usize,
    transport_mode: Arc<RwLock<OpenAITransportMode>>,
    websocket_cooldowns: Arc<RwLock<HashMap<String, Instant>>>,
    websocket_failure_streaks: Arc<RwLock<HashMap<String, u32>>>,
    /// Persistent WebSocket connection for incremental continuation
    persistent_ws: Arc<Mutex<Option<PersistentWsState>>>,
    /// Browser-backed ChatGPT state for web-only models such as GPT-5.6 Pro.
    chatgpt_web: Arc<chatgpt_web::ChatGptWebState>,
    /// True when this runtime was created without API credentials. It can still
    /// serve browser-backed models and upgrades in place after a successful login.
    browser_only: Arc<AtomicBool>,
}

impl OpenAIProvider {
    pub(crate) fn supports_extended_prompt_cache_retention(model_id: &str) -> bool {
        jcode_base::provider::openai::supports_extended_prompt_cache_retention(model_id)
    }

    fn effective_prompt_cache_retention<'a>(
        model_id: &str,
        configured: Option<&'a str>,
    ) -> Option<&'a str> {
        configured
            .or_else(|| Self::supports_extended_prompt_cache_retention(model_id).then_some("24h"))
    }

    pub fn new(credentials: CodexCredentials) -> Self {
        Self::new_inner(credentials, false)
    }

    /// Construct the OpenAI runtime for browser-backed models when no Codex or
    /// platform API credential exists. API models remain unavailable until a
    /// later auth refresh successfully loads credentials.
    pub fn new_browser_only() -> Self {
        Self::new_inner(
            CodexCredentials {
                access_token: String::new(),
                refresh_token: String::new(),
                id_token: None,
                account_id: None,
                expires_at: None,
            },
            true,
        )
    }

    fn new_inner(credentials: CodexCredentials, browser_only: bool) -> Self {
        let credential_mode = if browser_only {
            OpenAICredentialMode::Auto
        } else {
            OpenAICredentialMode::from_runtime_env(jcode_provider_core::DualAuthProvider::OpenAI)
        };
        let credentials = if browser_only {
            credentials
        } else {
            match credential_mode {
                OpenAICredentialMode::Auto => credentials,
                OpenAICredentialMode::OAuth | OpenAICredentialMode::ApiKey => {
                    load_credentials_for_mode(credential_mode).unwrap_or(credentials)
                }
            }
        };

        // Check for model override from environment
        let mut model = if browser_only {
            CHATGPT_WEB_MODEL.to_string()
        } else {
            std::env::var("JCODE_OPENAI_MODEL")
                .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
                .trim()
                .to_string()
        };
        if !is_chatgpt_web_model(&model)
            && !jcode_base::provider::known_openai_model_ids()
                .iter()
                .any(|known| known == &model)
        {
            jcode_base::logging::info(&format!(
                "Warning: '{}' is not supported; falling back to '{}'",
                model, DEFAULT_MODEL
            ));
            model = DEFAULT_MODEL.to_string();
        }

        let prompt_cache_key = std::env::var("JCODE_OPENAI_PROMPT_CACHE_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let prompt_cache_retention = std::env::var("JCODE_OPENAI_PROMPT_CACHE_RETENTION")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let prompt_cache_retention = match prompt_cache_retention.as_deref() {
            Some("in_memory") | Some("24h") => prompt_cache_retention,
            Some(other) => {
                jcode_base::logging::info(&format!(
                    "Warning: Unsupported JCODE_OPENAI_PROMPT_CACHE_RETENTION '{}'; expected 'in_memory' or '24h'",
                    other
                ));
                None
            }
            None => None,
        };
        let max_output_tokens = Self::load_max_output_tokens();
        let reasoning_effort = jcode_base::config::config()
            .provider
            .openai_reasoning_effort
            .as_deref()
            .and_then(Self::normalize_reasoning_effort);
        let service_tier = Self::load_service_tier(
            jcode_base::config::config()
                .provider
                .openai_service_tier
                .as_deref(),
        );
        let transport_mode = OpenAITransportMode::from_config(
            jcode_base::config::config()
                .provider
                .openai_transport
                .as_deref(),
        );
        let native_compaction_mode = OpenAINativeCompactionMode::from_config(
            &jcode_base::config::config()
                .provider
                .openai_native_compaction_mode,
        );
        let native_compaction_threshold_tokens = jcode_base::config::config()
            .provider
            .openai_native_compaction_threshold_tokens
            .max(1000);

        Self {
            client: jcode_provider_core::shared_http_client(),
            credentials: Arc::new(RwLock::new(credentials)),
            credential_mode: Arc::new(RwLock::new(credential_mode)),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key,
            prompt_cache_retention,
            max_output_tokens,
            reasoning_effort: Arc::new(StdRwLock::new(reasoning_effort)),
            service_tier: Arc::new(StdRwLock::new(service_tier)),
            native_compaction_mode,
            native_compaction_threshold_tokens,
            transport_mode: Arc::new(RwLock::new(transport_mode)),
            websocket_cooldowns: Arc::clone(&WEBSOCKET_COOLDOWNS),
            websocket_failure_streaks: Arc::clone(&WEBSOCKET_FAILURE_STREAKS),
            persistent_ws: Arc::new(Mutex::new(None)),
            chatgpt_web: Arc::new(chatgpt_web::ChatGptWebState::new()),
            browser_only: Arc::new(AtomicBool::new(browser_only)),
        }
    }

    fn is_browser_only(&self) -> bool {
        self.browser_only.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn reload_credentials_now(&self) {
        let mode = self
            .credential_mode
            .try_read()
            .map(|mode| *mode)
            .unwrap_or(OpenAICredentialMode::Auto);
        if let Ok(credentials) = load_credentials_for_mode(mode) {
            match self.credentials.try_write() {
                Ok(mut guard) => {
                    *guard = credentials;
                    self.browser_only.store(false, AtomicOrdering::Release);
                }
                Err(_) => {
                    jcode_base::logging::info(
                        "OpenAI credentials were updated on disk, but the in-memory credential lock was busy; async refresh will retry",
                    );
                }
            }
        }

        self.clear_persistent_ws_try("credentials reloaded");
    }

    pub(crate) fn set_credential_mode(&self, mode: OpenAICredentialMode) -> Result<()> {
        let credentials = load_credentials_for_mode(mode)?;
        match self.credentials.try_write() {
            Ok(mut guard) => {
                *guard = credentials;
                self.browser_only.store(false, AtomicOrdering::Release);
            }
            Err(_) => {
                anyhow::bail!(
                    "Cannot change OpenAI credential mode while a request is in progress"
                );
            }
        }
        match self.credential_mode.try_write() {
            Ok(mut guard) => {
                *guard = mode;
            }
            Err(_) => {
                anyhow::bail!(
                    "Cannot change OpenAI credential mode while a request is in progress"
                );
            }
        }
        self.clear_persistent_ws_try("OpenAI credential mode changed");
        // Keep the runtime provider identity in sync with the explicit credential
        // choice so UI surfaces report the auth method requests will actually use.
        // `Auto` leaves the existing identity untouched.
        if let Some(route) = mode.auth_route(jcode_provider_core::DualAuthProvider::OpenAI) {
            jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", route.runtime_provider_key());
        }
        // Drop any cached auth snapshot so surfaces that still consult the cheap
        // cached probe (auto-mode resolution, usage availability, account labels)
        // re-derive from the new credential choice on their next read instead of
        // lingering on a snapshot taken before the switch.
        jcode_base::auth::AuthStatus::invalidate_cached_status();
        Ok(())
    }

    pub(crate) fn credential_mode_snapshot(&self) -> OpenAICredentialMode {
        self.credential_mode
            .try_read()
            .map(|mode| *mode)
            .unwrap_or(OpenAICredentialMode::Auto)
    }

    fn clear_persistent_ws_try(&self, reason: &str) {
        if let Ok(mut persistent_ws) = self.persistent_ws.try_lock() {
            if persistent_ws.is_some() {
                jcode_base::logging::info(&format!(
                    "Clearing persistent OpenAI WS state: {}",
                    reason
                ));
            }
            *persistent_ws = None;
        }
    }

    async fn clear_persistent_ws(&self, reason: &str) {
        let mut persistent_ws = self.persistent_ws.lock().await;
        if persistent_ws.is_some() {
            jcode_base::logging::info(&format!("Clearing persistent OpenAI WS state: {}", reason));
        }
        *persistent_ws = None;
    }

    fn is_chatgpt_mode(credentials: &CodexCredentials) -> bool {
        !credentials.refresh_token.is_empty() || credentials.id_token.is_some()
    }

    fn should_prefer_websocket(model: &str) -> bool {
        !model.trim().is_empty()
    }

    fn normalize_reasoning_effort(raw: &str) -> Option<String> {
        let value = raw.trim().to_lowercase();
        if value.is_empty() {
            return None;
        }
        match value.as_str() {
            // `swarm` is a UI sentinel meaning "max effort + use the swarm tool".
            // We keep it stored so the UI/session reflect it and the agent injects
            // the swarm directive; it is translated to a real effort at request time
            // by `api_reasoning_effort`.
            "none" | "low" | "medium" | "high" | "xhigh" | "swarm" | "swarm-deep" => Some(value),
            other => {
                jcode_base::logging::info(&format!(
                    "Warning: Unsupported OpenAI reasoning effort '{}'; expected none|low|medium|high|xhigh. Using 'xhigh'.",
                    other
                ));
                Some("xhigh".to_string())
            }
        }
    }

    /// Translate a stored reasoning effort into the value sent to the API.
    /// The `swarm` sentinel maps to the strongest real effort (`xhigh`).
    fn api_reasoning_effort(effort: Option<&str>) -> Option<String> {
        match effort {
            Some(e) if jcode_base::prompt::is_swarm_effort(e) => Some("xhigh".to_string()),
            other => other.map(|e| e.to_string()),
        }
    }

    fn native_compaction_threshold_for_context_window(
        &self,
        context_window: usize,
    ) -> Option<usize> {
        if self.native_compaction_mode != OpenAINativeCompactionMode::Auto {
            return None;
        }
        Some(
            self.native_compaction_threshold_tokens
                .max(1000)
                .min(context_window.max(1000)),
        )
    }

    fn parse_max_output_tokens(raw: Option<&str>) -> Option<u32> {
        let raw = match raw {
            Some(value) => value.trim(),
            None => return Some(DEFAULT_MAX_OUTPUT_TOKENS),
        };
        if raw.is_empty() {
            return Some(DEFAULT_MAX_OUTPUT_TOKENS);
        }
        match raw.parse::<u32>() {
            Ok(0) => None,
            Ok(value) => Some(value),
            Err(_) => {
                jcode_base::logging::warn(&format!(
                    "Invalid JCODE_OPENAI_MAX_OUTPUT_TOKENS='{}'; using default {}",
                    raw, DEFAULT_MAX_OUTPUT_TOKENS
                ));
                Some(DEFAULT_MAX_OUTPUT_TOKENS)
            }
        }
    }

    fn normalize_service_tier(raw: &str) -> Result<Option<String>> {
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Ok(None);
        }

        match value.as_str() {
            "fast" | "priority" => Ok(Some("priority".to_string())),
            "flex" => Ok(Some("flex".to_string())),
            "default" | "auto" | "none" | "off" => Ok(None),
            other => anyhow::bail!(
                "Unsupported OpenAI service tier '{}'; expected priority|fast|flex|default|off",
                other
            ),
        }
    }

    fn load_service_tier(raw: Option<&str>) -> Option<String> {
        let raw = raw?;
        match Self::normalize_service_tier(raw) {
            Ok(value) => value,
            Err(err) => {
                jcode_base::logging::warn(&format!(
                    "{}; ignoring configured service tier override",
                    err
                ));
                None
            }
        }
    }

    fn load_max_output_tokens() -> Option<u32> {
        let raw = std::env::var("JCODE_OPENAI_MAX_OUTPUT_TOKENS").ok();
        let parsed = Self::parse_max_output_tokens(raw.as_deref());
        if raw.is_some() {
            match parsed {
                Some(value) => jcode_base::logging::info(&format!(
                    "OpenAI max_output_tokens configured to {}",
                    value
                )),
                None => jcode_base::logging::info(
                    "OpenAI max_output_tokens disabled (JCODE_OPENAI_MAX_OUTPUT_TOKENS=0)",
                ),
            }
        }
        parsed
    }

    fn responses_url(credentials: &CodexCredentials) -> String {
        let base = if Self::is_chatgpt_mode(credentials) {
            // ChatGPT/Codex OAuth backend is fixed; a custom base only applies
            // to API-key usage of the native Responses API.
            CHATGPT_API_BASE.to_string()
        } else {
            Self::resolve_api_base()
        };
        format!("{}/{}", base.trim_end_matches('/'), RESPONSES_PATH)
    }

    /// Resolve the OpenAI Responses API base URL for **API-key** mode.
    ///
    /// Defaults to `https://api.openai.com/v1`, but honors a user override so
    /// the native `openai-api` provider can target a local/proxied Responses
    /// API endpoint (issue #343). Checked in order:
    /// `JCODE_OPENAI_API_BASE`, `OPENAI_BASE_URL`, `OPENAI_API_BASE`.
    ///
    /// The override must be an absolute `http(s)://` URL; anything else is
    /// logged and ignored so a malformed value never silently breaks requests.
    /// A `/responses` suffix is not expected here (it is appended by callers),
    /// so a trailing `/responses` is trimmed to avoid `.../responses/responses`.
    pub(crate) fn resolve_api_base() -> String {
        jcode_base::provider::openai::resolve_api_base()
    }

    fn responses_ws_url(credentials: &CodexCredentials) -> String {
        let base = Self::responses_url(credentials);
        base.replace("https://", "wss://")
            .replace("http://", "ws://")
    }

    fn responses_compact_url(credentials: &CodexCredentials) -> String {
        format!("{}/compact", Self::responses_url(credentials))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "request construction threads explicit per-request OpenAI settings without hidden state"
    )]
    fn build_response_request(
        model_id: &str,
        instructions: String,
        input: &[Value],
        api_tools: &[Value],
        is_chatgpt_mode: bool,
        max_output_tokens: Option<u32>,
        reasoning_effort: Option<&str>,
        service_tier: Option<&str>,
        prompt_cache_key: Option<&str>,
        prompt_cache_retention: Option<&str>,
        native_compaction_threshold: Option<usize>,
    ) -> Value {
        let mut tools = api_tools.to_vec();
        // The hosted `image_generation` tool is only available to general
        // ChatGPT/GPT models on the Responses backend. Codex models
        // (`*-codex*`) reject unknown hosted tools, so don't attach it for them.
        if is_chatgpt_mode && model_supports_image_generation(model_id) {
            tools.push(serde_json::json!({ "type": "image_generation" }));
        }

        let mut request = serde_json::json!({
            "model": model_id,
            "instructions": instructions,
            "input": input,
            "tools": tools,
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
        });

        if !is_chatgpt_mode && let Some(max_output_tokens) = max_output_tokens {
            request["max_output_tokens"] = serde_json::json!(max_output_tokens);
        }

        if let Some(effort) = reasoning_effort {
            request["reasoning"] = serde_json::json!({ "effort": effort });
        }

        if let Some(service_tier) = service_tier {
            request["service_tier"] = serde_json::json!(service_tier);
        }

        if let Some(compact_threshold) = native_compaction_threshold {
            request["context_management"] = serde_json::json!([
                {
                    "type": "compaction",
                    "compact_threshold": compact_threshold,
                }
            ]);
        }

        if !is_chatgpt_mode {
            if let Some(key) = prompt_cache_key {
                request["prompt_cache_key"] = serde_json::json!(key);
            }
            if let Some(retention) =
                Self::effective_prompt_cache_retention(model_id, prompt_cache_retention)
            {
                request["prompt_cache_retention"] = serde_json::json!(retention);
            }
        }

        request
    }

    async fn model_id(&self) -> String {
        let current = self.model.read().await.clone();
        let availability = jcode_base::provider::model_availability_for_account(&current);

        match availability.state {
            jcode_base::provider::AccountModelAvailabilityState::Unavailable => {
                if let Some(detail) = availability.reason {
                    jcode_base::logging::info(&format!(
                        "Model '{}' currently unavailable ({}); selecting fallback",
                        current, detail
                    ));
                }
                if let Some(fallback) = jcode_base::provider::get_best_available_openai_model()
                    && fallback != current
                {
                    jcode_base::logging::info(&format!(
                        "Model '{}' not available for account; falling back to '{}'",
                        current, fallback
                    ));
                    {
                        let mut w = self.model.write().await;
                        *w = fallback.clone();
                    }
                    self.clear_persistent_ws(
                        "automatic OpenAI model fallback changed the response chain",
                    )
                    .await;
                    return fallback;
                }
            }
            jcode_base::provider::AccountModelAvailabilityState::Unknown => {
                if jcode_base::provider::should_refresh_openai_model_catalog()
                    && jcode_base::provider::begin_openai_model_catalog_refresh()
                {
                    let is_chatgpt_mode = {
                        let creds = self.credentials.read().await;
                        Self::is_chatgpt_mode(&creds)
                    };
                    // Go through the expiry-aware helper so a token that is
                    // about to (or already did) expire gets refreshed before
                    // the catalog request instead of guaranteeing a 401. Fall
                    // back to the raw snapshot if refresh fails; the fetch
                    // will then fail and finish the in-flight marker.
                    let token = match openai_access_token(&self.credentials).await {
                        Ok(token) => token,
                        Err(_) => self.credentials.read().await.access_token.clone(),
                    };
                    jcode_base::provider::refresh_openai_model_catalog_in_background(
                        token,
                        is_chatgpt_mode,
                        "openai-request-setup",
                    );
                }
            }
            jcode_base::provider::AccountModelAvailabilityState::Available => {}
        }

        current.strip_suffix("[1m]").unwrap_or(&current).to_string()
    }

    fn diagnostic_persistent_ws_summary(&self) -> String {
        match self.persistent_ws.try_lock() {
            Ok(guard) => guard
                .as_ref()
                .map(|state| state.diag_snapshot().log_fields())
                .unwrap_or_else(|| PersistentWsDiagSnapshot::absent().log_fields()),
            Err(_) => "persistent_ws=busy".to_string(),
        }
    }

    pub fn diagnostic_state_summary(&self) -> String {
        let transport_mode = self
            .transport_mode
            .try_read()
            .map(|mode| mode.as_str().to_string())
            .unwrap_or_else(|_| "busy".to_string());
        format!(
            "transport_mode={} {}",
            transport_mode,
            self.diagnostic_persistent_ws_summary()
        )
    }
}

#[path = "openai/stream.rs"]
mod stream;

#[cfg(test)]
use self::openai_stream_runtime::try_persistent_ws_continuation;
use self::openai_stream_runtime::{PersistentWsResult, is_retryable_error, openai_access_token};

use self::stream::{OpenAIResponsesStream, parse_openai_response_event};
#[cfg(test)]
use self::stream::{handle_openai_output_item, parse_text_wrapped_tool_call};

mod chatgpt_web;
#[path = "openai_provider_impl.rs"]
mod openai_provider_impl;
#[path = "openai_stream_runtime.rs"]
mod openai_stream_runtime;

#[path = "openai/websocket_health.rs"]
mod websocket_health;

use self::websocket_health::{
    WEBSOCKET_COMPLETION_TIMEOUT_SECS, WEBSOCKET_FALLBACK_NOTICE,
    WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS, classify_websocket_fallback_reason,
    is_stream_activity_event, is_websocket_activity_payload, is_websocket_fallback_notice,
    is_websocket_first_activity_payload, record_websocket_fallback, record_websocket_success,
    summarize_websocket_fallback_reason, websocket_activity_timeout_kind,
    websocket_cooldown_remaining, websocket_next_activity_timeout_secs_with_completion,
};
#[cfg(test)]
use self::websocket_health::{
    WEBSOCKET_MODEL_COOLDOWN_BASE_SECS, WEBSOCKET_MODEL_COOLDOWN_MAX_SECS, WebsocketFallbackReason,
    clear_websocket_cooldown, normalize_transport_model, set_websocket_cooldown,
    websocket_cooldown_for_streak, websocket_next_activity_timeout_secs,
    websocket_remaining_timeout_secs,
};

#[cfg(test)]
#[path = "openai_tests.rs"]
mod tests;
