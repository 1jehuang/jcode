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

/// Timing labels are deliberately static, never payloads, URLs or errors.
struct PushTiming {
    stage: &'static str,
    outcome: &'static str,
    start: Instant,
}

impl PushTiming {
    fn new(stage: &'static str) -> Self {
        Self {
            stage,
            outcome: "cancelled",
            start: Instant::now(),
        }
    }

    fn finish(mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}

impl Drop for PushTiming {
    fn drop(&mut self) {
        crate::logging::info(&format!(
            "[CLOUD_TIMING] operation=mobile_push stage={} outcome={} duration_ms={}",
            self.stage,
            self.outcome,
            self.start.elapsed().as_millis()
        ));
    }
}
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
        .map(|t| truncate_chars(&plain_inline(t), 120))
        .or_else(|| lookup_session_title(session_id));
    // An answer that ends by asking the user something is waiting on them.
    let (kind, summary) = match kind {
        PushKind::TurnDone => match trailing_question(body) {
            Some(question) => (PushKind::NeedsInput, question),
            None => (kind, summarize_answer(body)),
        },
        PushKind::NeedsInput => (
            kind,
            trailing_question(body).unwrap_or_else(|| summarize_answer(body)),
        ),
        PushKind::TurnError => (kind, summarize_error(body)),
        PushKind::Wake => (kind, summarize_wake(body)),
    };
    let collapsed = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = if collapsed.is_empty() {
        match kind {
            PushKind::TurnDone => "Turn finished".to_string(),
            PushKind::TurnError => "Turn failed".to_string(),
            PushKind::NeedsInput => "Waiting for your input".to_string(),
            PushKind::Wake => "Session update".to_string(),
        }
    } else {
        truncate_words(&collapsed, BODY_MAX_CHARS)
    };
    // Lead the title with a per-kind marker so the kinds are told apart at a
    // glance on the lock screen, not only by the small subtitle.
    let title = Some(format!(
        "{} {}",
        kind_marker(kind),
        title.as_deref().unwrap_or("jcode")
    ));
    PushPayload {
        kind,
        session_id: session_id.to_string(),
        session_title: title,
        body,
        // Message ids are random u64s. JSON numbers above 2^53 lose precision
        // in JavaScript and the API rejects them, so fold into the safe range.
        turn_id: turn_id.map(|id| id & JS_SAFE_INT_MASK),
    }
}

const JS_SAFE_INT_MASK: u64 = (1 << 53) - 1;

pub fn kind_marker(kind: PushKind) -> &'static str {
    match kind {
        PushKind::TurnDone => "\u{2705}",   // white check mark
        PushKind::TurnError => "\u{274C}",  // cross mark
        PushKind::NeedsInput => "\u{2753}", // question mark
        PushKind::Wake => "\u{23F0}",       // alarm clock
    }
}

fn lookup_session_title(session_id: &str) -> Option<String> {
    if cfg!(test) || session_id.is_empty() {
        return None;
    }
    crate::process_title::terminal_display_title_for_id(session_id)
        .map(|t| plain_inline(t.trim()))
        .filter(|t| !t.is_empty())
        .map(|t| truncate_chars(&t, 120))
}

/// Strip inline Markdown so a notification reads as plain prose.
fn plain_inline(text: &str) -> String {
    static LINK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static EMPH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let link = LINK.get_or_init(|| regex::Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap());
    let emph = EMPH.get_or_init(|| {
        regex::Regex::new(r"\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|~~([^~]+)~~").unwrap()
    });
    let text = link.replace_all(text, "$1");
    let text = emph.replace_all(&text, "$1$2$3$4");
    text.replace('`', "")
}

/// Turn Markdown into plain lines: drop code blocks, tables, rules and
/// headings markers, list bullets and inline emphasis.
fn plain_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.is_empty() {
            if !in_fence && out.last().is_some_and(|l: &String| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        if line.starts_with('|')
            || line
                .chars()
                .all(|c| matches!(c, '-' | '*' | '_' | '=' | ' '))
        {
            continue;
        }
        // Headings are labels ("Summary"), not content.
        if line.starts_with('#') {
            continue;
        }
        let line = line.trim_start_matches('>').trim();
        let line = strip_list_marker(line);
        let line = plain_inline(line);
        let line = line.trim();
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

fn strip_list_marker(line: &str) -> &str {
    for marker in ["- [ ] ", "- [x] ", "- ", "* ", "+ ", "• "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest;
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest;
        }
    }
    line
}

/// Lead of a final answer: the first paragraph, which by convention carries
/// the conclusion. Bullet items are joined into one sentence-like line.
fn summarize_answer(text: &str) -> String {
    let lines = plain_lines(text);
    let mut para = Vec::new();
    for line in &lines {
        if line.is_empty() {
            if !para.is_empty() && para.iter().map(|l: &&String| l.len()).sum::<usize>() >= 40 {
                break;
            }
            continue;
        }
        para.push(line);
    }
    join_sentences(para.iter().map(|l| l.as_str()))
}

fn join_sentences<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        if !out.is_empty() {
            if !out.ends_with(['.', '!', '?', ':', ';']) {
                out.push('.');
            }
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

/// If the answer ends by asking the user a question, return that question.
fn trailing_question(text: &str) -> Option<String> {
    let lines = plain_lines(text);
    let last = lines.iter().rev().find(|l| !l.is_empty())?;
    if !last.trim_end_matches(['*', '_', ')']).ends_with('?') {
        return None;
    }
    // Keep the last sentence, plus the one before if it sets up the question.
    let mut start = 0;
    for (i, c) in last.char_indices() {
        if matches!(c, '.' | '!' | '?') && i + 1 < last.len() - 1 {
            let next = last[i + c.len_utf8()..].chars().next();
            if next.is_some_and(char::is_whitespace) {
                start = i + c.len_utf8();
            }
        }
    }
    let question = last[start..].trim();
    (question.chars().count() >= 8).then(|| question.to_string())
}

/// Errors arrive as `context: cause: cause`. Drop JSON blobs and noise, keep
/// the most specific readable cause.
fn summarize_error(text: &str) -> String {
    static JSON: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let json = JSON.get_or_init(|| regex::Regex::new(r"\{.*\}").unwrap());
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Pull a human message out of an embedded provider JSON error if present.
    let message = json.find(&flat).and_then(|m| {
        serde_json::from_str::<serde_json::Value>(m.as_str())
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.get("message"))
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
    });
    let stripped = json.replace_all(&flat, "");
    let mut parts: Vec<&str> = stripped
        .split(": ")
        .map(|p| p.trim().trim_end_matches([':', ',']))
        .filter(|p| !p.is_empty())
        .collect();
    parts.dedup();
    let head = parts.join(": ");
    match message {
        Some(m) if !head.contains(m.as_str()) => {
            if head.is_empty() {
                m
            } else {
                format!("{m} ({head})")
            }
        }
        _ => head,
    }
}

/// Wake notifications are agent-facing Markdown. Rewrite the known shapes
/// into one readable line and strip instructions meant for the model.
fn summarize_wake(text: &str) -> String {
    if let Some(task) = crate::message::parse_background_task_notification_markdown(text) {
        let label = task
            .display_name
            .clone()
            .unwrap_or_else(|| task.tool_name.clone());
        let status = task.status.trim_start_matches(['✓', '✗', '↻', ' ']).trim();
        let duration = humanize_secs(&task.duration);
        let mut line = format!("{label} {status} in {duration}");
        if status == "failed"
            && task.exit_label.starts_with("exit ")
            && task.exit_label != "exit n/a"
        {
            line.push_str(&format!(" ({})", task.exit_label));
        }
        let detail = task
            .failure_summary
            .as_deref()
            .map(plain_inline)
            .or_else(|| task.preview.as_deref().and_then(last_meaningful_line));
        if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
            line.push_str(": ");
            line.push_str(detail.trim());
        }
        return line;
    }
    if let Some(rest) = text.trim().strip_prefix("**Background task stalled** ") {
        let header = rest.lines().next().unwrap_or_default();
        let label = header
            .split(" · ")
            .nth(1)
            .map(plain_inline)
            .unwrap_or_default();
        let label = label
            .split(" (")
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let idle = header
            .split("no output or progress for ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .map(humanize_secs)
            .unwrap_or_default();
        return format!("{label} has been silent for {idle} and may be stuck")
            .trim()
            .to_string();
    }
    let lines: Vec<String> = plain_lines(text)
        .into_iter()
        .filter(|l| !l.is_empty() && !l.contains("action=\"") && !l.starts_with("Full output:"))
        .collect();
    join_sentences(lines.iter().map(String::as_str))
}

fn last_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rev()
        .find(|l| l.chars().filter(|c| c.is_alphanumeric()).count() >= 4)
        .map(|l| truncate_chars(l, 120))
}

fn humanize_secs(value: &str) -> String {
    let Ok(secs) = value.trim_end_matches('s').parse::<f64>() else {
        return value.to_string();
    };
    let secs = secs.round() as u64;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => match secs % 60 {
            0 => format!("{}m", secs / 60),
            s => format!("{}m {s}s", secs / 60),
        },
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// Truncate at a word boundary so previews never end mid-word.
fn truncate_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    let cut = head
        .rfind(char::is_whitespace)
        .filter(|&i| i >= head.len() * 2 / 3)
        .unwrap_or(head.len());
    let mut out = head[..cut]
        .trim_end_matches([',', ';', ':', ' ', '-'])
        .to_string();
    out.push('…');
    out
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

/// True while the API recently reported no registered devices.
pub(crate) fn devices_suppressed() -> bool {
    SUPPRESSION
        .lock()
        .map(|s| s.is_suppressed(Instant::now()))
        .unwrap_or(false)
}

fn env_opted_out(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

pub(crate) fn enabled() -> bool {
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

pub(crate) fn credentials() -> Option<(String, String)> {
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
    if cfg!(test) {
        return;
    }
    let requested = kind;
    if !enabled() {
        record(
            requested,
            None,
            body,
            "skipped",
            Some("disabled by config or JCODE_MOBILE_PUSH"),
            None,
        );
        return;
    }
    if SUPPRESSION
        .lock()
        .map(|s| s.is_suppressed(Instant::now()))
        .unwrap_or(false)
    {
        crate::logging::debug("mobile push suppressed: no registered devices");
        record(
            requested,
            None,
            body,
            "skipped",
            Some("no registered devices (10 min back-off)"),
            None,
        );
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        record(
            requested,
            None,
            body,
            "skipped",
            Some("no tokio runtime"),
            None,
        );
        return;
    };
    let payload = build_payload(kind, session_id, session_title, body, turn_id);
    let source = body.to_string();
    handle.spawn(async move {
        let Some((key, base)) = credentials() else {
            record(
                requested,
                Some(&payload),
                &source,
                "skipped",
                Some("not signed in (no JCODE_API_KEY)"),
                None,
            );
            return;
        };
        let mut attempt = send(&key, &base, &payload).await;
        // Transient network failures (DNS, reset, timeout) get one retry.
        if attempt.0 == "error" && attempt.2.is_none() {
            tokio::time::sleep(Duration::from_secs(3)).await;
            attempt = send(&key, &base, &payload).await;
            if let Some(detail) = attempt.1.as_mut() {
                detail.push_str(" (after retry)");
            }
        }
        let (outcome, detail, status) = attempt;
        record(
            requested,
            Some(&payload),
            &source,
            outcome,
            detail.as_deref(),
            status,
        );
    });
}

/// Record a turn that deliberately did not push, so the history explains
/// missing notifications too.
pub fn note_skipped(kind: PushKind, reason: &str) {
    if !cfg!(test) {
        record(kind, None, "", "skipped", Some(reason), None);
    }
}

/// Append one line per push attempt to `~/.jcode/logs/mobile-push.jsonl`:
/// what was requested, the exact payload sent, a source excerpt, and the
/// outcome. This is the push history used to debug notification quality.
fn record(
    requested: PushKind,
    payload: Option<&PushPayload>,
    source: &str,
    outcome: &str,
    detail: Option<&str>,
    http_status: Option<u16>,
) {
    use std::io::Write;
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "requested_kind": requested,
        "outcome": outcome,
        "detail": detail,
        "http_status": http_status,
        "payload": payload,
        "source_chars": source.chars().count(),
        "source_excerpt": truncate_chars(source, 1500),
    });
    crate::logging::info(&format!(
        "mobile push {:?} {outcome}{}",
        payload.map(|p| p.kind).unwrap_or(requested),
        detail.map(|d| format!(": {d}")).unwrap_or_default()
    ));
    let Some(dir) = dirs::home_dir().map(|h| h.join(".jcode").join("logs")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("mobile-push.jsonl");
    // Keep the history bounded: rotate once past 5 MB.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("mobile-push.1.jsonl"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}");
    }
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
    let total = PushTiming::new("total");
    let result = send_mobile_resume_inner(key, base, payload).await;
    total.finish(if result.is_ok() { "delivered" } else { "error" });
    result
}

async fn send_mobile_resume_inner(
    key: &str,
    base: &str,
    payload: &PushPayload,
) -> anyhow::Result<String> {
    let http = PushTiming::new("http_send");
    let result = crate::provider::shared_http_client()
        .post(format!("{base}/push/send"))
        .bearer_auth(key)
        .json(&payload)
        .timeout(SEND_TIMEOUT)
        .send()
        .await;
    http.finish(if result.is_ok() { "response" } else { "error" });
    let response = result?.error_for_status()?;
    let body = PushTiming::new("body");
    let result = response.json::<SendReply>().await;
    body.finish(if result.is_ok() { "decoded" } else { "error" });
    let reply = result?;
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

async fn send(
    key: &str,
    base: &str,
    payload: &PushPayload,
) -> (&'static str, Option<String>, Option<u16>) {
    let total = PushTiming::new("total");
    let result = send_inner(key, base, payload).await;
    total.finish(result.0);
    result
}

async fn send_inner(
    key: &str,
    base: &str,
    payload: &PushPayload,
) -> (&'static str, Option<String>, Option<u16>) {
    let http = PushTiming::new("http_send");
    let result = crate::provider::shared_http_client()
        .post(format!("{base}/push/send"))
        .bearer_auth(key)
        .json(payload)
        .timeout(SEND_TIMEOUT)
        .send()
        .await;
    http.finish(if result.is_ok() { "response" } else { "error" });
    match result {
        Ok(resp) if resp.status().is_success() => {
            let status = resp.status().as_u16();
            let body = PushTiming::new("body");
            let result = resp.json::<SendReply>().await;
            body.finish(if result.is_ok() { "decoded" } else { "error" });
            let reply = match result {
                Ok(reply) => reply,
                Err(error) => {
                    return (
                        "error",
                        Some(format!("invalid reply: {error}")),
                        Some(status),
                    );
                }
            };
            let devices = reply.devices;
            if let Ok(mut s) = SUPPRESSION.lock() {
                s.record_reply(devices, Instant::now());
            }
            let detail = format!("sent={:?} devices={:?}", reply.sent, devices);
            let outcome = match (reply.sent, devices) {
                (_, Some(0)) => "no_devices",
                (Some(n), _) if n > 0 => "delivered",
                _ => "undelivered",
            };
            (outcome, Some(detail), Some(status))
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            // Keep the API's error body: it says exactly which field was rejected.
            let timing = PushTiming::new("body");
            let result = resp.text().await;
            timing.finish(if result.is_ok() { "read" } else { "error" });
            let body = result.unwrap_or_default();
            crate::logging::warn(&format!(
                "mobile push {:?} rejected: HTTP {status}: {}",
                payload.kind,
                truncate_chars(&body, 300)
            ));
            ("rejected", Some(truncate_chars(&body, 500)), Some(status))
        }
        Err(error) => {
            let detail = crate::util::format_error_chain(&anyhow::Error::from(error));
            crate::logging::warn(&format!("mobile push failed: {detail}"));
            ("error", Some(detail), None)
        }
    }
}

#[cfg(test)]
#[path = "mobile_push_http_tests.rs"]
mod http_tests;

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
                "session_title": "\u{2705} Fix login",
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
        assert_eq!(v["session_title"], "\u{2753} jcode");
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
    fn mobile_push_turn_id_is_js_safe() {
        let p = build_payload(
            PushKind::TurnDone,
            "s",
            None,
            "ok",
            Some(8338987288606337570),
        );
        assert!(p.turn_id.unwrap() <= (1 << 53) - 1);
        assert_eq!(
            build_payload(PushKind::TurnDone, "s", None, "ok", Some(42)).turn_id,
            Some(42)
        );
    }

    #[test]
    fn mobile_push_answer_uses_lead_paragraph_as_plain_text() {
        let answer = "## Summary\n\nFixed the **login** bug in `auth.rs`; all 12 tests pass.\n\n- detail one\n- detail two\n\n```rust\nfn x() {}\n```";
        let p = build_payload(PushKind::TurnDone, "s", Some("Fix `login`"), answer, None);
        assert_eq!(p.body, "Fixed the login bug in auth.rs; all 12 tests pass.");
        assert_eq!(p.session_title.as_deref(), Some("\u{2705} Fix login"));
        for (kind, body, marker) in [
            (PushKind::TurnDone, "done", "\u{2705}"),
            (PushKind::TurnDone, "Should I push?", "\u{2753}"),
            (PushKind::TurnError, "boom", "\u{274C}"),
            (PushKind::Wake, "bash completed", "\u{23F0}"),
        ] {
            let t = build_payload(kind, "s", Some("T"), body, None)
                .session_title
                .unwrap();
            assert_eq!(t, format!("{marker} T"));
        }
        let bullets = "- Updated parser\n- Added tests\n- Shipped v2 to production servers today";
        assert_eq!(
            build_payload(PushKind::TurnDone, "s", None, bullets, None).body,
            "Updated parser. Added tests. Shipped v2 to production servers today"
        );
    }

    #[test]
    fn mobile_push_trailing_question_becomes_needs_input() {
        let p = build_payload(
            PushKind::TurnDone,
            "s",
            None,
            "I found two options.\n\nShould I use OAuth or a password?",
            None,
        );
        assert_eq!(p.kind, PushKind::NeedsInput);
        assert_eq!(p.body, "Should I use OAuth or a password?");
        let p = build_payload(
            PushKind::TurnDone,
            "s",
            None,
            "Done. Want me to deploy it?",
            None,
        );
        assert_eq!(
            (p.kind, p.body.as_str()),
            (PushKind::NeedsInput, "Want me to deploy it?")
        );
    }

    #[test]
    fn mobile_push_errors_and_wakes_are_readable() {
        let e = r#"Provider request failed: HTTP 529: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(
            build_payload(PushKind::TurnError, "s", None, e, None).body,
            "Overloaded (Provider request failed: HTTP 529)"
        );
        let bg = "**Background task** `abc123` · `bash` · ✓ completed · 754.2s · exit 0\n\n```text\nCompiling...\ntest result: ok. 42 passed\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`";
        assert_eq!(
            build_payload(PushKind::Wake, "s", None, bg, None).body,
            "bash completed in 12m 34s: test result: ok. 42 passed"
        );
        let long = "word ".repeat(60);
        let p = build_payload(PushKind::Wake, "s", None, &long, None);
        assert!(p.body.ends_with("word…"));
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
