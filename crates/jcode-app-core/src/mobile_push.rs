//! Fire-and-forget mobile push notifications through the Jcode API.
//!
//! Contract: `~/jcode-mobile/docs/PUSH.md`. The server POSTs
//! `{base}/push/send` with the account API key; the API Worker fans out to
//! APNs. Sends never block or fail a turn.

use crate::subscription_catalog::{
    DEFAULT_JCODE_API_BASE, JCODE_API_BASE_ENV, JCODE_API_KEY_ENV, JCODE_ENV_FILE,
};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Finished turns shorter than this do not push (user is likely watching).
pub const MIN_TURN_DURATION_FOR_PUSH: Duration = Duration::from_secs(20);
/// Back-off after the API reports no registered devices.
pub const NO_DEVICES_SUPPRESS: Duration = Duration::from_secs(10 * 60);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const BODY_MAX_CHARS: usize = 180;
pub const OPT_OUT_ENV: &str = "JCODE_MOBILE_PUSH";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    TurnDone,
    TurnError,
    NeedsInput,
    Wake,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushPayload {
    pub kind: PushKind,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

pub fn build_payload(
    kind: PushKind,
    session_id: &str,
    session_title: Option<&str>,
    body: &str,
    turn_id: Option<u64>,
) -> PushPayload {
    let title = session_title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| truncate_chars(t, 120));
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = if collapsed.is_empty() {
        match kind {
            PushKind::TurnDone => "Turn finished".to_string(),
            PushKind::TurnError => "Turn failed".to_string(),
            PushKind::NeedsInput => "Waiting for your input".to_string(),
            PushKind::Wake => "Session update".to_string(),
        }
    } else {
        truncate_chars(&collapsed, BODY_MAX_CHARS)
    };
    PushPayload {
        kind,
        session_id: session_id.to_string(),
        session_title: title,
        body,
        turn_id,
    }
}

/// Last `max` chars are most useful for an assistant reply preview? No: the
/// start of a sentence reads better, so keep the head.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Tracks the "no devices" back-off window.
#[derive(Debug, Default)]
pub struct Suppression {
    until: Option<Instant>,
}

impl Suppression {
    pub fn is_suppressed(&self, now: Instant) -> bool {
        self.until.is_some_and(|until| now < until)
    }

    pub fn record_reply(&mut self, devices: Option<u64>, now: Instant) {
        self.until = match devices {
            Some(0) => Some(now + NO_DEVICES_SUPPRESS),
            _ => None,
        };
    }
}

static SUPPRESSION: Mutex<Suppression> = Mutex::new(Suppression { until: None });

fn env_opted_out(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

fn enabled() -> bool {
    let opt_out = match std::env::var(OPT_OUT_ENV) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => {
            crate::logging::warn(&format!("invalid mobile push opt-out setting: {error}"));
            return false;
        }
    };
    if env_opted_out(opt_out.as_deref()) {
        return false;
    }
    crate::config::config().server.mobile_push != Some(false)
}

fn credentials() -> Option<(String, String)> {
    let key = crate::provider_catalog::load_api_key_from_env_or_config(
        JCODE_API_KEY_ENV,
        JCODE_ENV_FILE,
    )?;
    let base = crate::provider_catalog::load_env_value_from_env_or_config(
        JCODE_API_BASE_ENV,
        JCODE_ENV_FILE,
    )
    .filter(|b| !b.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_JCODE_API_BASE.to_string());
    Some((key, base.trim().trim_end_matches('/').to_string()))
}

/// Threshold for sessions a `jcode-ios` client attached to: push every turn.
pub const MIN_TURN_DURATION_FOR_IOS_SESSION: Duration = Duration::ZERO;
pub const IOS_CLIENT_PREFIX: &str = "jcode-ios";

static IOS_SESSIONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static CANCELLED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn set_insert(set: &Mutex<Option<HashSet<String>>>, id: &str) {
    if let Ok(mut g) = set.lock() {
        g.get_or_insert_with(HashSet::new).insert(id.to_string());
    }
}

fn set_remove(set: &Mutex<Option<HashSet<String>>>, id: &str) -> bool {
    let mut guard = set.lock().unwrap_or_else(|error| {
        crate::logging::warn("recovering poisoned mobile push session state");
        error.into_inner()
    });
    guard.as_mut().is_some_and(|sessions| sessions.remove(id))
}

/// Record that a client named `client_name` attached to `session_id`.
pub fn note_client_attached(session_id: &str, client_name: &str) {
    if client_name.starts_with(IOS_CLIENT_PREFIX) {
        set_insert(&IOS_SESSIONS, session_id);
    }
}

fn is_ios_session(session_id: &str) -> bool {
    let guard = IOS_SESSIONS.lock().unwrap_or_else(|error| {
        crate::logging::warn("recovering poisoned mobile push iOS session state");
        error.into_inner()
    });
    guard
        .as_ref()
        .is_some_and(|sessions| sessions.contains(session_id))
}

/// A user cancel was requested; the ending turn must not push.
pub fn mark_cancelled(session_id: &str) {
    set_insert(&CANCELLED, session_id);
}

pub fn clear_cancelled(session_id: &str) {
    set_remove(&CANCELLED, session_id);
}

/// Consume the cancel marker for a finished turn.
pub fn take_cancelled(session_id: &str) -> bool {
    set_remove(&CANCELLED, session_id)
}

fn threshold_for(ios_session: bool) -> Duration {
    if ios_session {
        MIN_TURN_DURATION_FOR_IOS_SESSION
    } else {
        MIN_TURN_DURATION_FOR_PUSH
    }
}

/// Whether a finished turn of this session and duration should push.
pub fn should_push_turn_done(session_id: &str, elapsed: Duration) -> bool {
    elapsed >= threshold_for(is_ios_session(session_id))
}

/// Queue a push. Returns immediately; never errors.
pub fn notify(
    kind: PushKind,
    session_id: &str,
    session_title: Option<&str>,
    body: &str,
    turn_id: Option<u64>,
) {
    if cfg!(test) || !enabled() {
        return;
    }
    if SUPPRESSION
        .lock()
        .map(|s| s.is_suppressed(Instant::now()))
        .unwrap_or(false)
    {
        crate::logging::debug("mobile push suppressed: no registered devices");
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let payload = build_payload(kind, session_id, session_title, body, turn_id);
    handle.spawn(async move {
        let Some((key, base)) = credentials() else {
            return;
        };
        send(&key, &base, &payload).await;
    });
}

#[derive(serde::Deserialize)]
struct SendReply {
    #[serde(default)]
    sent: Option<u64>,
    #[serde(default)]
    devices: Option<u64>,
}

/// Explicit user-requested handoff. Unlike automatic turn notifications this
/// bypasses duration thresholds, opt-outs, and the no-devices suppression cache.
pub async fn request_mobile_resume(
    session_id: &str,
    title: Option<&str>,
) -> anyhow::Result<String> {
    let (key, base) = credentials().ok_or_else(|| {
        anyhow::anyhow!("Sign in first with `jcode account login`, then retry /mobile.")
    })?;
    let payload = build_payload(
        PushKind::Wake,
        session_id,
        title,
        "Tap to resume this session on Jcode Cloud.",
        None,
    );
    send_mobile_resume(&key, &base, &payload).await
}

async fn send_mobile_resume(
    key: &str,
    base: &str,
    payload: &PushPayload,
) -> anyhow::Result<String> {
    let response = reqwest::Client::new()
        .post(format!("{base}/push/send"))
        .bearer_auth(key)
        .json(&payload)
        .timeout(SEND_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    let reply = response.json::<SendReply>().await?;
    match (reply.devices, reply.sent) {
        (Some(0), _) => anyhow::bail!(
            "No mobile devices registered. Open the Jcode app, sign in to the same account, and enable notifications, then retry /mobile."
        ),
        (_, Some(sent)) if sent > 0 => Ok(format!(
            "Mobile notification sent to {sent} device(s). Tap it to open this session."
        )),
        _ => anyhow::bail!(
            "No mobile notification was delivered. Check notification permissions in the Jcode app, then retry /mobile."
        ),
    }
}

async fn send(key: &str, base: &str, payload: &PushPayload) {
    let client = match reqwest::Client::builder().timeout(SEND_TIMEOUT).build() {
        Ok(c) => c,
        Err(error) => {
            crate::logging::warn(&format!("mobile push client build failed: {error}"));
            return;
        }
    };
    let result = client
        .post(format!("{base}/push/send"))
        .bearer_auth(key)
        .json(payload)
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => {
            let reply = match resp.json::<SendReply>().await {
                Ok(reply) => reply,
                Err(error) => {
                    crate::logging::warn(&format!("invalid mobile push reply: {error}"));
                    return;
                }
            };
            let devices = reply.devices;
            if let Ok(mut s) = SUPPRESSION.lock() {
                s.record_reply(devices, Instant::now());
            }
            crate::logging::debug(&format!(
                "mobile push {:?} session={} sent={:?} devices={:?}",
                payload.kind, payload.session_id, reply.sent, devices
            ));
        }
        Ok(resp) => crate::logging::warn(&format!(
            "mobile push {:?} rejected: HTTP {}",
            payload.kind,
            resp.status()
        )),
        Err(error) => crate::logging::warn(&format!("mobile push failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_marker_survives_poisoned_state() {
        let state = Mutex::new(Some(HashSet::from(["cancelled".to_string()])));
        let result = std::panic::catch_unwind(|| {
            let _guard = state.lock().unwrap();
            panic!("simulate interrupted state update");
        });
        assert!(result.is_err());
        assert!(set_remove(&state, "cancelled"));
        assert!(!set_remove(&state, "cancelled"));
    }

    #[tokio::test]
    async fn explicit_resume_reports_delivery_and_preserves_session_target() {
        use std::io::{Read, Write};
        for (status, body, expected) in [
            (200, r#"{"sent":1,"devices":1}"#, "Mobile notification sent"),
            (
                200,
                r#"{"sent":0,"devices":0}"#,
                "No mobile devices registered",
            ),
            (
                200,
                r#"{"sent":0,"devices":1}"#,
                "No mobile notification was delivered",
            ),
            (200, "not json", "error decoding response body"),
            (401, "{}", "401 Unauthorized"),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let len: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let reason = if status == 200 { "OK" } else { "Unauthorized" };
                write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                String::from_utf8(request).unwrap()
            });
            let payload = build_payload(
                PushKind::Wake,
                "session_resume",
                Some("My task"),
                "Tap to resume",
                None,
            );
            let result = send_mobile_resume("test-key", &base, &payload).await;
            let message = match result {
                Ok(message) => message,
                Err(error) => error.to_string(),
            };
            assert!(
                message.contains(expected),
                "{message:?} should contain {expected:?}"
            );
            let request = server.join().unwrap();
            assert!(request.starts_with("POST /push/send "));
            assert!(
                request
                    .to_lowercase()
                    .contains("authorization: bearer test-key")
            );
            let body = request.split_once("\r\n\r\n").unwrap().1;
            let json: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(json["session_id"], "session_resume");
            assert_eq!(json["kind"], "wake");
        }
    }

    #[test]
    fn mobile_push_payload_serializes_per_contract() {
        let p = build_payload(
            PushKind::TurnDone,
            "session_x",
            Some(" Fix login "),
            "Done.\n\nAll   tests pass",
            Some(42),
        );
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "kind": "turn_done",
                "session_id": "session_x",
                "session_title": "Fix login",
                "body": "Done. All tests pass",
                "turn_id": 42
            })
        );
    }

    #[test]
    fn mobile_push_payload_defaults_and_truncation() {
        let p = build_payload(PushKind::NeedsInput, "s", Some("  "), "", None);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "needs_input");
        assert!(v.get("session_title").is_none());
        assert!(v.get("turn_id").is_none());
        assert_eq!(v["body"], "Waiting for your input");
        let long = "a".repeat(500);
        let p = build_payload(PushKind::TurnError, "s", None, &long, None);
        assert_eq!(p.body.chars().count(), BODY_MAX_CHARS);
        assert_eq!(
            serde_json::to_value(PushKind::Wake).unwrap(),
            serde_json::json!("wake")
        );
    }

    #[test]
    fn mobile_push_suppression_window() {
        let now = Instant::now();
        let mut s = Suppression::default();
        assert!(!s.is_suppressed(now));
        s.record_reply(Some(0), now);
        assert!(s.is_suppressed(now + Duration::from_secs(60)));
        assert!(!s.is_suppressed(now + NO_DEVICES_SUPPRESS));
        s.record_reply(Some(0), now);
        s.record_reply(Some(2), now);
        assert!(!s.is_suppressed(now));
        s.record_reply(None, now);
        assert!(!s.is_suppressed(now));
    }

    #[test]
    fn mobile_push_opt_out_and_threshold() {
        assert!(env_opted_out(Some("0")));
        assert!(env_opted_out(Some("false")));
        assert!(!env_opted_out(Some("1")));
        assert!(!env_opted_out(None));
        let sid = "session_mobile_push_test_threshold";
        assert!(!should_push_turn_done(sid, Duration::ZERO));
        assert!(!should_push_turn_done(
            sid,
            MIN_TURN_DURATION_FOR_PUSH - Duration::from_nanos(1)
        ));
        assert!(should_push_turn_done(sid, MIN_TURN_DURATION_FOR_PUSH));
        assert_eq!(threshold_for(true), Duration::ZERO);
    }

    #[test]
    fn mobile_push_ios_sessions_and_cancel_markers() {
        let sid = "session_mobile_push_test_ios";
        assert!(!should_push_turn_done(sid, Duration::from_secs(1)));
        note_client_attached(sid, "jcode-desktop/1.0");
        assert!(!should_push_turn_done(sid, Duration::from_secs(1)));
        note_client_attached(sid, "jcode-ios/0.3.0");
        assert!(should_push_turn_done(sid, Duration::ZERO));

        let c = "session_mobile_push_test_cancel";
        assert!(!take_cancelled(c));
        mark_cancelled(c);
        assert!(take_cancelled(c));
        assert!(!take_cancelled(c));
        mark_cancelled(c);
        clear_cancelled(c);
        assert!(!take_cancelled(c));
    }
}
