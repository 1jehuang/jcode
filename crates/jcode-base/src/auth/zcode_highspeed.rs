//! ZCode HighSpeed acceleration cards ("1.5x usage on frontier models").
//!
//! Protocol facts verified against the official ZCode client and the
//! independent reference implementation
//! https://github.com/TriDefender/zcode-api (protocol knowledge only — no
//! code copied; that project is CC BY-NC-SA 4.0).
//!
//! - `POST {origin}/api/v1/highspeed/draw` attempts to win a limited-time
//!   acceleration card (~5 min window) bound to a task id. The server token
//!   bucket admits ~1 draw/s with burst 8; 429 backs off 5s.
//! - A held card reroutes eligible requests to
//!   `{origin}/api/v1/highspeed/anthropic/v1/messages` with the zcode JWT in
//!   the auth slot, `X-Bigmodel-Authorization: Bearer {codingPlanAuth}`,
//!   `Bigmodel-Target-Type: PERSONAL`, and `X-Highspeed-Card-ID`.
//! - Carded requests carry the card's model (requests are rewritten to it),
//!   and a flash request must never take a non-flash card (draw rejects
//!   flash models outright with HTTP 400 biz 3001 on some accounts).
//! - 503 / biz 3404 = account not yet in the rollout; negative-cache 10 min.
//! - Quota-dead cards answer HTTP 200 with a top-level `code` envelope
//!   (e.g. `{"code":1005,"msg":"exceed quota limit"}`) — the caller treats
//!   any 200 JSON body carrying a top-level `code` as a card rejection and
//!   falls back to the normal route.

use crate::auth::zcode::load_credentials;
use crate::logging;
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;

pub const HIGHSPEED_ORIGIN: &str = "https://zcode.z.ai";
const DRAW_PATH: &str = "/api/v1/highspeed/draw";
const DRAW_TIMEOUT: Duration = Duration::from_secs(10);
const DRAW_RETRY: Duration = Duration::from_secs(1);
const IDLE_WINDOW: Duration = Duration::from_secs(5 * 60);
const UNAVAILABLE_WINDOW: Duration = Duration::from_secs(10 * 60);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(5);
const TRANSPORT_FAILURE_BACKOFF: Duration = Duration::from_secs(60);
const CARD_MIN_REMAINING_MS: i64 = 10_000;
/// Model-family prefixes that trigger draws / accept cards.
pub const HIGHSPEED_MODEL_PREFIXES: &[&str] = &["glm-5.3", "glm-5.2", "glm-5.1"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighspeedCard {
    pub card_id: String,
    pub task_id: String,
    pub model: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct HighspeedAuth {
    /// zcode plan JWT (the `token` field of `zcode_oauth.json`).
    pub jwt: String,
    /// Coding-plan credential string (`{apiKeyId}.{secret}`).
    pub coding_plan_auth: String,
}

impl HighspeedAuth {
    pub fn from_credentials() -> Option<Self> {
        let creds = load_credentials().ok()?;
        let jwt = creds.token.trim().to_string();
        let key = creds.coding_plan_api_key.as_deref()?.trim().to_string();
        if jwt.is_empty() || key.is_empty() {
            return None;
        }
        Some(Self {
            jwt,
            coding_plan_auth: key,
        })
    }
}

#[derive(Debug, Deserialize)]
struct DrawEnvelope {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<DrawData>,
}

#[derive(Debug, Deserialize)]
struct DrawData {
    #[serde(default)]
    card: Option<DrawCardWire>,
}

#[derive(Debug, Deserialize)]
struct DrawCardWire {
    #[serde(default)]
    card_id: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    issued_at: Option<u64>,
    #[serde(default)]
    expires_at: Option<u64>,
}

#[derive(Debug, Default)]
struct HighspeedState {
    card: Option<HighspeedCard>,
    unavailable_until_ms: u64,
    next_attempt_at_ms: u64,
    blocked_models: std::collections::HashMap<String, u64>,
    loop_until_ms: u64,
    latest: Option<(String, String, HighspeedAuth)>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn is_eligible_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    HIGHSPEED_MODEL_PREFIXES
        .iter()
        .any(|p| m == *p || m.starts_with(&format!("{p}-")))
}

fn is_flash(model: &str) -> bool {
    model.to_ascii_lowercase().contains("flash")
}

/// Carded inference URL — carded requests reroute here, NOT the normal
/// coding-plan provider URL.
pub fn highspeed_llm_url() -> String {
    format!("{HIGHSPEED_ORIGIN}/api/v1/highspeed/anthropic/v1/messages")
}

/// True when the response means the carded route is unusable and the caller
/// must fall back to the normal route: HTTP 401, a non-200/non-5xx status
/// whose JSON body carries biz code 3402, or a 200 JSON body with a
/// top-level `code` (quota-dead card envelope; a legitimate Anthropic
/// success body never carries a top-level `code`). SSE bodies are never
/// sniffed.
pub fn is_highspeed_card_rejection(status: u16, content_type: Option<&str>, body_head: &str) -> bool {
    if status == 401 {
        return true;
    }
    if !(200..500).contains(&status) || status >= 500 {
        return false;
    }
    let is_json = content_type
        .map(|c| c.contains("application/json"))
        .unwrap_or(false)
        || body_head.trim_start().starts_with('{');
    if !is_json {
        return false;
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body_head) else {
        return false;
    };
    let code = json.get("code").and_then(|v| v.as_i64());
    if status == 200 {
        code.is_some()
    } else {
        code == Some(3402)
    }
}

/// Shared process-wide manager (single card slot, like the official client).
pub struct HighspeedManager {
    origin: &'static str,
    state: Mutex<HighspeedState>,
    wake: tokio::sync::Notify,
    loop_alive: AtomicU64,
}

impl Default for HighspeedManager {
    fn default() -> Self {
        Self::new()
    }
}

static MANAGER: std::sync::OnceLock<Arc<HighspeedManager>> = std::sync::OnceLock::new();

pub fn shared() -> Arc<HighspeedManager> {
    Arc::clone(MANAGER.get_or_init(|| Arc::new(HighspeedManager::new())))
}

impl HighspeedManager {
    fn new() -> Self {
        Self {
            origin: HIGHSPEED_ORIGIN,
            state: Mutex::new(HighspeedState::default()),
            wake: tokio::sync::Notify::new(),
            loop_alive: AtomicU64::new(0),
        }
    }

    /// The active card usable for THIS model (a flash request must never take
    /// a non-flash card: mounting rewrites the request model to card.model).
    pub async fn active_card_for(&self, model: &str) -> Option<HighspeedCard> {
        let mut state = self.state.lock().await;
        let now = now_ms();
        if let Some(card) = &state.card {
            if card.expires_at_ms <= now + CARD_MIN_REMAINING_MS as u64 {
                state.card = None;
            }
        }
        let card = state.card.as_ref()?;
        if is_flash(model) == is_flash(&card.model) {
            Some(card.clone())
        } else {
            None
        }
    }

    /// Drop the card after an upstream rejection.
    pub async fn drop_card(&self, reason: &str) {
        let mut state = self.state.lock().await;
        if state.card.take().is_some() {
            logging::info(&format!("ZCode highspeed: card dropped ({reason})"));
            let loop_until = state.loop_until_ms;
            drop(state);
            if now_ms() < loop_until {
                self.ensure_loop();
            }
        }
    }

    /// Fire-and-forget draw trigger — call on every eligible coding-plan
    /// request. Stamps the idle window with the latest request params and
    /// (re)ignites the background loop.
    pub async fn on_request(&self, model: &str, session_task_id: Option<&str>) {
        if !is_eligible_model(model) {
            return;
        }
        let Some(auth) = HighspeedAuth::from_credentials() else {
            return;
        };
        let now = now_ms();
        {
            let mut state = self.state.lock().await;
            if let Some(&blocked) = state.blocked_models.get(&model.trim().to_ascii_lowercase())
                && now < blocked
            {
                return;
            }
            if let Some(card) = &state.card
                && card.expires_at_ms <= now
            {
                state.card = None;
            }
            state.latest = Some((
                model.trim().to_string(),
                session_task_id.unwrap_or("").to_string(),
                auth,
            ));
            state.loop_until_ms = now + IDLE_WINDOW.as_millis() as u64;
            if let Some(card) = &state.card
                && card.expires_at_ms.saturating_sub(now) >= CARD_MIN_REMAINING_MS as u64
            {
                return;
            }
        }
        self.ensure_loop();
    }

    /// Extend the idle window WITHOUT re-stamping model/auth params (a stream
    /// still generating is not "idle").
    pub async fn note_activity(&self) {
        let mut state = self.state.lock().await;
        state.loop_until_ms = now_ms() + IDLE_WINDOW.as_millis() as u64;
    }

    fn ensure_loop(&self) {
        if self.loop_alive.load(Ordering::SeqCst) == 0 {
            self.loop_alive.store(1, Ordering::SeqCst);
            let this = Arc::clone(&shared());
            tokio::spawn(async move {
                this.run_loop().await;
            });
        }
        self.wake.notify_one();
    }

    async fn run_loop(&self) {
        loop {
            let mut wait = DRAW_RETRY;
            {
                let mut state = self.state.lock().await;
                let now = now_ms();
                if let Some(card) = &state.card
                    && card.expires_at_ms.saturating_sub(now) >= CARD_MIN_REMAINING_MS as u64
                {
                    state.card = None; // handled below via early return
                }
                if state.card.is_some() {
                    // held card — loop parks; wake() re-ignites when dropped
                    self.loop_alive.store(0, Ordering::SeqCst);
                    return;
                }
                if now >= state.loop_until_ms {
                    self.loop_alive.store(0, Ordering::SeqCst);
                    logging::info(
                        "ZCode highspeed: draw loop idle-stopped (5min without eligible requests)",
                    );
                    return;
                }
                if now < state.next_attempt_at_ms || now < state.unavailable_until_ms {
                    wait = Duration::from_millis(
                        state
                            .next_attempt_at_ms
                            .max(state.unavailable_until_ms)
                            .saturating_sub(now)
                            .min(5_000),
                    );
                } else if let Some((model, task_id, auth)) = state.latest.clone() {
                    drop(state);
                    self.draw(model, task_id, auth).await;
                    continue;
                }
            }
            let _ = tokio::time::timeout(wait, self.wake.notified()).await;
        }
    }

    async fn draw(&self, model: String, task_id: String, auth: HighspeedAuth) {
        let url = format!("{}{DRAW_PATH}", self.origin);
        let effective_task = if task_id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            task_id
        };
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .timeout(DRAW_TIMEOUT)
            .header("content-type", "application/json")
            .header(
                "Authorization",
                if auth.jwt.starts_with("Bearer ") {
                    auth.jwt.clone()
                } else {
                    format!("Bearer {}", auth.jwt)
                },
            )
            .header("X-Bigmodel-Authorization", format!("Bearer {}", auth.coding_plan_auth))
            .header("Bigmodel-Target-Type", "PERSONAL")
            .header("x-request-id", Uuid::new_v4().to_string())
            .json(&serde_json::json!({
                "task_id": effective_task,
                "provider": "builtin:zai-coding-plan",
                "model": model,
            }))
            .send()
            .await;
        let now = now_ms();
        let mut state = self.state.lock().await;
        let resp = match resp {
            Ok(r) => r,
            Err(err) => {
                state.next_attempt_at_ms = now + TRANSPORT_FAILURE_BACKOFF.as_millis() as u64;
                logging::info(&format!(
                    "ZCode highspeed: draw failed (network/timeout: {err}) — retrying later"
                ));
                return;
            }
        };
        let status = resp.status().as_u16();
        if status == 429 {
            state.next_attempt_at_ms = now + RATE_LIMIT_BACKOFF.as_millis() as u64;
            logging::info("ZCode highspeed: draw rate-limited (429) — backing off 5s");
            return;
        }
        let body: DrawEnvelope = match resp.json().await {
            Ok(b) => b,
            Err(_) => {
                state.unavailable_until_ms = now + UNAVAILABLE_WINDOW.as_millis() as u64;
                logging::info(&format!(
                    "ZCode highspeed: draw unavailable (HTTP {status}, invalid JSON) — negative-cached 10min"
                ));
                return;
            }
        };
        if status != 200 {
            if status == 400 {
                // Parameter-level rejection — the account is fine; block only
                // this model.
                state
                    .blocked_models
                    .insert(model.trim().to_ascii_lowercase(), now + UNAVAILABLE_WINDOW.as_millis() as u64);
                logging::info(&format!(
                    "ZCode highspeed: draw rejected (HTTP 400{}) — model {model} ineligible for draw",
                    body
                        .msg
                        .as_deref()
                        .map(|m| format!(" {m}"))
                        .unwrap_or_default(),
                ));
            } else {
                state.unavailable_until_ms = now + UNAVAILABLE_WINDOW.as_millis() as u64;
                logging::info(&format!(
                    "ZCode highspeed: draw unavailable (HTTP {status}) — negative-cached 10min"
                ));
            }
            return;
        }
        if let Some(code) = body.code
            && code != 0
        {
            // 3404 = account-level "highspeed temporarily unavailable".
            state.unavailable_until_ms = now + UNAVAILABLE_WINDOW.as_millis() as u64;
            logging::info(&format!(
                "ZCode highspeed: draw rejected (code={code} {}) — negative-cached 10min",
                body.msg.as_deref().unwrap_or(""),
            ));
            return;
        }
        if let Some(wire) = body.data.and_then(|d| d.card)
            && let (Some(card_id), Some(card_model), Some(expires_at)) =
                (wire.card_id, wire.model, wire.expires_at)
            && expires_at > now
        {
            state.card = Some(HighspeedCard {
                card_id,
                task_id: wire.task_id.unwrap_or(effective_task),
                model: card_model,
                issued_at_ms: wire.issued_at.unwrap_or(now),
                expires_at_ms: expires_at,
            });
            let window = (expires_at.saturating_sub(now)) / 1000;
            let model_name = state.card.as_ref().map(|c| c.model.as_str()).unwrap_or("?");
            logging::info(&format!(
                "ZCode highspeed: card drawn (model={model_name} window={window}s)"
            ));
        }
        // Miss (card=null) — retry at the heartbeat cadence until idle stop.
    }
}

/// Convenience: attach card headers to an existing header builder. The
/// coding-plan dual auth (`x-api-key` + `Authorization`) is replaced by the
/// zcode JWT pair, then the card headers are appended.
pub fn apply_highspeed_card_headers(
    headers: &mut reqwest::header::HeaderMap,
    card: &HighspeedCard,
    auth: &HighspeedAuth,
) {
    let bare = auth.jwt.trim_start_matches("Bearer ").trim();
    if let Ok(v) = reqwest::header::HeaderValue::from_str(bare) {
        headers.insert(reqwest::header::HeaderName::from_static("x-api-key"), v);
    }
    if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {bare}")) {
        headers.insert(
            reqwest::header::HeaderName::from_static("authorization"),
            val,
        );
    }
    for (name, value) in [
        (
            "x-bigmodel-authorization",
            format!("Bearer {}", auth.coding_plan_auth),
        ),
        ("bigmodel-target-type", "PERSONAL".to_string()),
        ("x-highspeed-card-id", card.card_id.clone()),
    ] {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            headers.insert(name, val);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_model_prefix_match() {
        assert!(is_eligible_model("glm-5.3"));
        assert!(is_eligible_model("glm-5.3-flash"));
        assert!(is_eligible_model("GLM-5.2 "));
        assert!(!is_eligible_model("claude-opus-5"));
        assert!(!is_eligible_model(""));
    }

    #[test]
    fn card_rejection_predicates() {
        assert!(is_highspeed_card_rejection(401, None, ""));
        assert!(is_highspeed_card_rejection(
            200,
            Some("application/json"),
            r#"{"code":1005,"msg":"exceed quota limit"}"#
        ));
        assert!(is_highspeed_card_rejection(
            400,
            Some("application/json"),
            r#"{"code":3402}"#
        ));
        // A legitimate Anthropic SSE stream is never a rejection.
        assert!(!is_highspeed_card_rejection(
            200,
            Some("text/event-stream"),
            "event: message_start"
        ));
        assert!(!is_highspeed_card_rejection(
            200,
            Some("application/json"),
            r#"{"id":"msg_1","content":[]}"#
        ));
    }

    #[test]
    fn carded_url_shape() {
        assert_eq!(
            highspeed_llm_url(),
            "https://zcode.z.ai/api/v1/highspeed/anthropic/v1/messages"
        );
    }

    #[test]
    fn flash_card_affinity() {
        let card = HighspeedCard {
            card_id: "c1".into(),
            task_id: "t1".into(),
            model: "glm-5.3".into(),
            issued_at_ms: 0,
            expires_at_ms: u64::MAX,
        };
        // flash request vs non-flash card handled by active_card_for; here we
        // just assert the flash classifier used by that gate.
        assert!(is_flash("glm-5.3-flash"));
        assert!(!is_flash("glm-5.3"));
        let _ = card;
    }
}
