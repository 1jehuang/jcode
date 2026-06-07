use jcode_message_types::StreamEvent;

pub const WEBSOCKET_FALLBACK_NOTICE: &str = "falling back from websockets to https transport";
pub const WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS: u64 = 8;
pub const WEBSOCKET_COMPLETION_TIMEOUT_SECS: u64 = 300;
pub const WEBSOCKET_MODEL_COOLDOWN_BASE_SECS: u64 = 60;
pub const WEBSOCKET_MODEL_COOLDOWN_MAX_SECS: u64 = 600;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebsocketFallbackReason {
    ConnectTimeout,
    FirstResponseTimeout,
    StreamTimeout,
    ServerRequestedHttps,
    ConnectFailed,
    StreamClosedEarly,
    WebsocketError,
}

impl WebsocketFallbackReason {
    pub fn summary(self) -> &'static str {
        match self {
            Self::ConnectTimeout => "connect timeout",
            Self::FirstResponseTimeout => "first response timeout",
            Self::StreamTimeout => "stream timeout",
            Self::ServerRequestedHttps => "server requested https",
            Self::ConnectFailed => "connect failed",
            Self::StreamClosedEarly => "stream closed early",
            Self::WebsocketError => "websocket error",
        }
    }
}

pub fn is_websocket_fallback_notice(data: &str) -> bool {
    // The proxy injects the fallback notice as a plain-text control frame, not
    // a structured Responses API event. A legitimate `response.*`/`error`
    // event can legitimately *contain* this phrase (for example inside
    // tool-call arguments when the model is editing source that mentions
    // websocket fallback), so a structured event must never be reinterpreted
    // as a transport control frame.
    if is_structured_response_event(data) {
        return false;
    }
    data.to_lowercase().contains(WEBSOCKET_FALLBACK_NOTICE)
}

pub fn is_stream_activity_event(_event: &StreamEvent) -> bool {
    true
}

/// Returns true when `data` parses as a structured Responses API stream event
/// (a JSON object whose `type` is a `response.*` event or a top-level `error`).
/// These frames carry model output and must be parsed as protocol events even
/// if their content happens to contain transport-control phrases.
pub fn is_structured_response_event(data: &str) -> bool {
    is_websocket_activity_payload(data)
}

pub fn is_websocket_activity_payload(data: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    let Some(kind) = value.get("type").and_then(|kind| kind.as_str()) else {
        return false;
    };
    kind.starts_with("response.") || kind == "error"
}

pub fn is_websocket_first_activity_payload(data: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    value
        .get("type")
        .and_then(|kind| kind.as_str())
        .map(|kind| !kind.is_empty())
        .unwrap_or(false)
}

pub fn websocket_remaining_timeout_secs(since: Instant, timeout_secs: u64) -> Option<u64> {
    let timeout = Duration::from_secs(timeout_secs);
    let elapsed = since.elapsed();
    if elapsed >= timeout {
        return None;
    }

    Some(timeout_secs.saturating_sub(elapsed.as_secs()).max(1))
}

pub fn websocket_next_activity_timeout_secs(
    ws_started_at: Instant,
    last_api_activity_at: Instant,
    saw_api_activity: bool,
) -> Option<u64> {
    if !saw_api_activity {
        websocket_remaining_timeout_secs(ws_started_at, WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS)
    } else {
        websocket_remaining_timeout_secs(last_api_activity_at, WEBSOCKET_COMPLETION_TIMEOUT_SECS)
    }
}

pub fn websocket_activity_timeout_kind(saw_api_activity: bool) -> &'static str {
    if saw_api_activity { "next" } else { "first" }
}

pub fn classify_websocket_fallback_reason(error: &str) -> WebsocketFallbackReason {
    let error = error.to_ascii_lowercase();
    if error.contains("connect timed out") {
        WebsocketFallbackReason::ConnectTimeout
    } else if error.contains("did not emit api activity within")
        || error.contains("timed out waiting for first websocket activity")
    {
        WebsocketFallbackReason::FirstResponseTimeout
    } else if error.contains("timed out waiting for next websocket activity")
        || error.contains("did not complete within")
    {
        WebsocketFallbackReason::StreamTimeout
    } else if error.contains("upgrade required")
        || error.contains("server requested fallback")
        || error.contains(WEBSOCKET_FALLBACK_NOTICE)
    {
        WebsocketFallbackReason::ServerRequestedHttps
    } else if error.contains("failed to connect websocket stream") {
        WebsocketFallbackReason::ConnectFailed
    } else if error.contains("ended before response.completed")
        || error.contains("closed before response.completed")
    {
        WebsocketFallbackReason::StreamClosedEarly
    } else {
        WebsocketFallbackReason::WebsocketError
    }
}

pub fn summarize_websocket_fallback_reason(error: &str) -> &'static str {
    classify_websocket_fallback_reason(error).summary()
}

fn websocket_cooldown_bounds_for_reason(reason: WebsocketFallbackReason) -> (u64, u64) {
    match reason {
        WebsocketFallbackReason::ServerRequestedHttps => (
            WEBSOCKET_MODEL_COOLDOWN_BASE_SECS.saturating_mul(5),
            WEBSOCKET_MODEL_COOLDOWN_MAX_SECS.saturating_mul(3),
        ),
        WebsocketFallbackReason::StreamTimeout => (
            WEBSOCKET_MODEL_COOLDOWN_BASE_SECS,
            WEBSOCKET_MODEL_COOLDOWN_MAX_SECS,
        ),
        WebsocketFallbackReason::ConnectTimeout
        | WebsocketFallbackReason::FirstResponseTimeout
        | WebsocketFallbackReason::ConnectFailed
        | WebsocketFallbackReason::StreamClosedEarly
        | WebsocketFallbackReason::WebsocketError => (
            (WEBSOCKET_MODEL_COOLDOWN_BASE_SECS / 2).max(1),
            (WEBSOCKET_MODEL_COOLDOWN_MAX_SECS / 2).max(1),
        ),
    }
}

pub fn normalize_transport_model(model: &str) -> Option<String> {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub async fn websocket_cooldown_remaining(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
) -> Option<Duration> {
    let key = normalize_transport_model(model)?;
    let now = Instant::now();

    {
        let guard = websocket_cooldowns.read().await;
        if let Some(until) = guard.get(&key)
            && *until > now
        {
            return Some(*until - now);
        }
    }

    let mut guard = websocket_cooldowns.write().await;
    if let Some(until) = guard.get(&key)
        && *until > now
    {
        return Some(*until - now);
    }
    if guard.get(&key).is_some() {
        guard.remove(&key);
    }
    None
}

pub async fn set_websocket_cooldown(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
) {
    let Some(key) = normalize_transport_model(model) else {
        return;
    };

    let cooldown = Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS);
    let until = Instant::now() + cooldown;
    let mut guard = websocket_cooldowns.write().await;
    guard.insert(key, until);
}

pub async fn set_websocket_cooldown_for(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
    cooldown: Duration,
) {
    let Some(key) = normalize_transport_model(model) else {
        return;
    };

    let until = Instant::now() + cooldown;
    let mut guard = websocket_cooldowns.write().await;
    guard.insert(key, until);
}

pub async fn clear_websocket_cooldown(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
) {
    let Some(key) = normalize_transport_model(model) else {
        return;
    };

    let mut guard = websocket_cooldowns.write().await;
    guard.remove(&key);
}

pub fn websocket_cooldown_for_streak(streak: u32, reason: WebsocketFallbackReason) -> Duration {
    let (base, max) = websocket_cooldown_bounds_for_reason(reason);
    let base = base as u128;
    let max = max as u128;
    let shift = streak.saturating_sub(1).min(16);
    let scaled = base.saturating_mul(1u128 << shift);
    Duration::from_secs(scaled.min(max) as u64)
}

pub async fn record_websocket_fallback(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    websocket_failure_streaks: &Arc<RwLock<HashMap<String, u32>>>,
    model: &str,
    reason: WebsocketFallbackReason,
) -> (u32, Duration) {
    let Some(key) = normalize_transport_model(model) else {
        return (0, websocket_cooldown_for_streak(1, reason));
    };

    let streak = {
        let mut guard = websocket_failure_streaks.write().await;
        let entry = guard.entry(key).or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    };

    let cooldown = websocket_cooldown_for_streak(streak, reason);
    set_websocket_cooldown_for(websocket_cooldowns, model, cooldown).await;
    (streak, cooldown)
}

pub async fn record_websocket_success(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    websocket_failure_streaks: &Arc<RwLock<HashMap<String, u32>>>,
    model: &str,
) {
    clear_websocket_cooldown(websocket_cooldowns, model).await;
    let Some(key) = normalize_transport_model(model) else {
        return;
    };
    let streak = {
        let mut guard = websocket_failure_streaks.write().await;
        guard.remove(&key).unwrap_or(0)
    };
    if streak > 0 {
        jcode_logging::info(&format!(
            "OpenAI websocket health reset for model='{}' after successful stream (previous streak={})",
            model, streak
        ));
    }
}
