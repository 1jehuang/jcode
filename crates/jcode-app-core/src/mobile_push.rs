//! Fire-and-forget mobile push notifications through the Jcode API.
//!
//! Contract: `~/jcode-mobile/docs/PUSH.md`. The server POSTs
//! `{base}/push/send` with the account API key; the API Worker fans out to
//! APNs. Sends never block or fail a turn.

use crate::subscription_catalog::{
    DEFAULT_JCODE_API_BASE, JCODE_API_BASE_ENV, JCODE_API_KEY_ENV, JCODE_ENV_FILE,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Finished turns shorter than this do not push (user is likely watching).
pub const MIN_TURN_DURATION_FOR_PUSH: Duration = Duration::from_secs(20);
/// Back-off after the API reports no registered devices.
pub const NO_DEVICES_SUPPRESS: Duration = Duration::from_secs(10 * 60);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
// Allow the TUI's three-second automatic failover countdown to restart work.
const ERROR_PUSH_GRACE: Duration = Duration::from_secs(10);
static PENDING_ERRORS: Mutex<Option<HashMap<String, Arc<AtomicBool>>>> = Mutex::new(None);

fn queue_pending_error(session_id: &str) -> Arc<AtomicBool> {
    let token = Arc::new(AtomicBool::new(false));
    if let Ok(mut pending) = PENDING_ERRORS.lock() {
        if let Some(previous) = pending
            .get_or_insert_with(HashMap::new)
            .insert(session_id.to_owned(), token.clone())
        {
            previous.store(true, Ordering::Release);
        }
    }
    token
}

fn finish_pending_error(session_id: &str, token: &Arc<AtomicBool>) -> bool {
    if let Ok(mut pending) = PENDING_ERRORS.lock()
        && let Some(map) = pending.as_mut()
        && map
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, token))
    {
        map.remove(session_id);
    }
    !token.load(Ordering::Acquire)
}

fn cancel_pending_error(session_id: &str) {
    if let Ok(mut pending) = PENDING_ERRORS.lock()
        && let Some(token) = pending.as_mut().and_then(|map| map.remove(session_id))
    {
        token.store(true, Ordering::Release);
    }
}

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
    /// The session's own name. The API shows it on the notification's second
    /// line, leaving `session_title` as a short where/what/how-long headline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
    /// Where the session runs: `Cloud` or this machine's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Repository (or directory) the session works in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// How long the turn that triggered this push ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
}

/// Context shown ahead of the session name so a notification says where it
/// came from before anything else.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushLabels {
    pub origin: Option<String>,
    pub repo: Option<String>,
    pub elapsed: Option<Duration>,
}

impl PushLabels {
    /// Labels for a live session. `working_dir` falls back to the saved
    /// session's directory when the caller does not have it at hand.
    pub fn for_session(
        session_id: &str,
        working_dir: Option<&str>,
        elapsed: Option<Duration>,
    ) -> Self {
        if cfg!(test) {
            return Self::default();
        }
        let saved;
        let dir = match working_dir {
            Some(dir) => Some(dir),
            None => {
                saved = lookup_session_working_dir(session_id);
                saved.as_deref()
            }
        };
        Self {
            origin: Some(origin_label().to_string()),
            repo: dir.and_then(|dir| repo_label(std::path::Path::new(dir))),
            elapsed,
        }
    }
}

const CLOUD_ORIGIN: &str = "Cloud";
const LABEL_SEPARATOR: &str = " \u{00B7} ";
const LABEL_MAX_CHARS: usize = 24;
const TITLE_MAX_CHARS: usize = 120;

/// `Cloud` on a managed Jcode Cloud machine, otherwise this machine's name.
pub(crate) fn origin_label() -> &'static str {
    static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LABEL.get_or_init(|| {
        if crate::mobile_runner::is_cloud_machine() {
            return CLOUD_ORIGIN.to_string();
        }
        let read = |path: &str| std::fs::read_to_string(path).ok();
        machine_label(
            read("/sys/devices/virtual/dmi/id/product_name").as_deref(),
            std::env::var("HOSTNAME")
                .ok()
                .or_else(|| read("/etc/hostname"))
                .as_deref(),
        )
    })
}

/// Short human name for a machine: the hardware model without its SKU code
/// (`XPS 14 DA14260` reads as `XPS 14`), else the hostname, else `Local`.
fn machine_label(product_name: Option<&str>, host_name: Option<&str>) -> String {
    // SKU codes mix letters and digits in one long token; model words do not.
    let is_sku = |token: &str| {
        token.chars().count() >= 5
            && token.chars().any(|c| c.is_ascii_digit())
            && token.chars().any(|c| c.is_alphabetic())
    };
    let placeholder = |name: &str| {
        let lower = name.to_ascii_lowercase();
        [
            "system product name",
            "to be filled",
            "default string",
            "virtual machine",
            "standard pc",
            "not specified",
        ]
        .iter()
        .any(|p| lower.contains(p))
    };
    let model = product_name
        .map(|name| {
            name.split_whitespace()
                .filter(|token| !is_sku(token))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|name| !name.is_empty() && !placeholder(name));
    let host = host_name
        .map(|host| host.trim().split('.').next().unwrap_or("").to_string())
        .filter(|host| !host.is_empty() && host != "localhost");
    truncate_chars(
        &model.or(host).unwrap_or_else(|| "Local".to_string()),
        LABEL_MAX_CHARS,
    )
}

/// Name of the repository containing `dir`, else the directory's own name.
/// The home directory and filesystem root identify nothing, so they are
/// left unlabeled.
fn repo_label(dir: &std::path::Path) -> Option<String> {
    let home = dirs::home_dir();
    let root = dir
        .ancestors()
        .take_while(|p| Some(*p) != home.as_deref())
        .find(|p| p.join(".git").exists())
        .unwrap_or(dir);
    if Some(root) == home.as_deref() {
        return None;
    }
    let name = root.file_name()?.to_string_lossy();
    let name = name.trim();
    (!name.is_empty()).then(|| truncate_chars(name, LABEL_MAX_CHARS))
}

fn lookup_session_working_dir(session_id: &str) -> Option<String> {
    if session_id.is_empty() {
        return None;
    }
    crate::session::Session::load_startup_stub(session_id)
        .ok()
        .and_then(|session| session.working_dir)
}

fn humanize_duration(elapsed: Duration) -> String {
    humanize_secs(&elapsed.as_secs().to_string())
}

pub fn build_payload(
    kind: PushKind,
    session_id: &str,
    session_title: Option<&str>,
    body: &str,
    turn_id: Option<u64>,
) -> PushPayload {
    build_payload_with(
        kind,
        session_id,
        session_title,
        body,
        turn_id,
        &PushLabels::for_session(session_id, None, None),
    )
}

pub fn build_payload_with(
    kind: PushKind,
    session_id: &str,
    session_title: Option<&str>,
    body: &str,
    turn_id: Option<u64>,
    labels: &PushLabels,
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
    let duration_secs = labels.elapsed.map(|elapsed| elapsed.as_secs());
    // Each line has one job. The headline says where the session runs, which
    // repo, what happened and how long it took, and is short enough to fit a
    // lock screen. The session name gets the second line to itself and the
    // body carries only the result.
    let clean_label = |label: &Option<String>| {
        label
            .as_deref()
            .map(|l| plain_inline(l.trim()))
            .filter(|l| !l.is_empty())
    };
    let origin = clean_label(&labels.origin);
    let repo = clean_label(&labels.repo);
    let run_time = labels.elapsed.map(humanize_duration);
    let status = match (kind_status(kind), run_time) {
        // A finished turn needs no word beyond its check mark and run time.
        (_, Some(time)) if kind == PushKind::TurnDone => format!("{} {time}", kind_marker(kind)),
        (word, Some(time)) => format!("{} {word}{LABEL_SEPARATOR}{time}", kind_marker(kind)),
        (word, None) => format!("{} {word}", kind_marker(kind)),
    };
    let headline = [origin.clone(), repo.clone(), Some(status)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(LABEL_SEPARATOR);
    let session_name = title;
    let title = Some(truncate_chars(&headline, TITLE_MAX_CHARS));
    PushPayload {
        kind,
        session_id: session_id.to_string(),
        session_title: title,
        session_name,
        body,
        // Message ids are random u64s. JSON numbers above 2^53 lose precision
        // in JavaScript and the API rejects them, so fold into the safe range.
        turn_id: turn_id.map(|id| id & JS_SAFE_INT_MASK),
        origin,
        repo,
        duration_secs,
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

/// The word that follows the marker in the headline.
fn kind_status(kind: PushKind) -> &'static str {
    match kind {
        PushKind::TurnDone => "Done",
        PushKind::TurnError => "Failed",
        PushKind::NeedsInput => "Needs input",
        PushKind::Wake => "Update",
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

fn cached_regex(
    cache: &'static std::sync::OnceLock<Option<regex::Regex>>,
    pattern: &str,
) -> Option<&'static regex::Regex> {
    cache
        .get_or_init(|| match regex::Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(error) => {
                crate::logging::warn(&format!("invalid mobile push summary regex: {error}"));
                None
            }
        })
        .as_ref()
}

/// Strip inline Markdown so a notification reads as plain prose.
fn plain_inline(text: &str) -> String {
    static LINK: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    static EMPH: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let link = cached_regex(&LINK, r"!?\[([^\]]*)\]\([^)]*\)");
    let emph = cached_regex(
        &EMPH,
        r"\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|~~([^~]+)~~",
    );
    let text = match link {
        Some(link) => link.replace_all(text, "$1"),
        None => std::borrow::Cow::Borrowed(text),
    };
    let text = match emph {
        Some(emph) => emph.replace_all(&text, "$1$2$3$4"),
        None => std::borrow::Cow::Borrowed(text.as_ref()),
    };
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
    // A provider failover prompt is machine-readable; say it in plain words.
    if let Some(prompt) = jcode_provider_core::failover::parse_failover_prompt_message(text) {
        return format!(
            "{} is unavailable. Open the session to continue on {}.",
            prompt.from_label, prompt.to_label
        );
    }
    static JSON: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(json) = cached_regex(&JSON, r"\{.*\}") else {
        return flat;
    };
    // Pull a human message out of an embedded provider JSON error if present.
    let message = json.find(&flat).and_then(|m| {
        match serde_json::from_str::<serde_json::Value>(m.as_str()) {
            Ok(value) => Some(value),
            Err(error) => {
                crate::logging::debug(&format!(
                    "mobile push error contains non-JSON braces: {error}"
                ));
                None
            }
        }
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
        // Incomplete optional wake headers intentionally render missing pieces empty.
        let header = rest.lines().next().unwrap_or("");
        let label = match header.split(" · ").nth(1) {
            Some(label) => plain_inline(label),
            None => String::new(),
        };
        let label = label.split(" (").next().unwrap_or("").trim().to_string();
        let idle = match header
            .split("no output or progress for ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
        {
            Some(idle) => humanize_secs(idle),
            None => String::new(),
        };
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

/// How long an identical error for the same session stays quiet.
const REPEAT_ERROR_QUIET: Duration = Duration::from_secs(10 * 60);

/// Remembers the last error pushed per session so a retry loop that keeps
/// failing the same way notifies once, not on every attempt.
#[derive(Debug, Default)]
pub struct RepeatGuard {
    last: std::collections::HashMap<String, (String, Instant)>,
}

impl RepeatGuard {
    /// True when this exact error was already pushed for the session inside
    /// the quiet window. Otherwise records it and returns false.
    pub fn is_repeat(&mut self, session_id: &str, body: &str, now: Instant) -> bool {
        self.last
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < REPEAT_ERROR_QUIET);
        match self.last.get(session_id) {
            Some((last, _)) if last == body => true,
            _ => {
                self.last
                    .insert(session_id.to_string(), (body.to_string(), now));
                false
            }
        }
    }

    /// Any other notification for the session ends the quiet window.
    pub fn clear(&mut self, session_id: &str) {
        self.last.remove(session_id);
    }
}

static REPEAT_ERRORS: std::sync::LazyLock<Mutex<RepeatGuard>> =
    std::sync::LazyLock::new(|| Mutex::new(RepeatGuard::default()));

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
    cancel_pending_error(session_id);
    set_insert(&CANCELLED, session_id);
}

pub fn clear_cancelled(session_id: &str) {
    cancel_pending_error(session_id);
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
    notify_turn(kind, session_id, session_title, body, turn_id, None, None);
}

/// Queue a push for a turn that just ended, labeled with how long it ran and
/// the directory it worked in.
pub fn notify_turn(
    kind: PushKind,
    session_id: &str,
    session_title: Option<&str>,
    body: &str,
    turn_id: Option<u64>,
    working_dir: Option<&str>,
    elapsed: Option<Duration>,
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
    let payload = build_payload_with(
        kind,
        session_id,
        session_title,
        body,
        turn_id,
        &PushLabels::for_session(session_id, working_dir, elapsed),
    );
    let source = body.to_string();
    let pending_error = if payload.kind == PushKind::TurnError {
        Some(queue_pending_error(session_id))
    } else {
        cancel_pending_error(session_id);
        None
    };
    handle.spawn(async move {
        if let Some(token) = pending_error {
            tokio::time::sleep(ERROR_PUSH_GRACE).await;
            if !finish_pending_error(&payload.session_id, &token) {
                record(
                    requested,
                    Some(&payload),
                    &source,
                    "skipped",
                    Some("session resumed or cancelled during error grace period"),
                    None,
                );
                return;
            }
        }
        if let Ok(mut guard) = REPEAT_ERRORS.lock() {
            if payload.kind != PushKind::TurnError {
                guard.clear(&payload.session_id);
            } else if guard.is_repeat(&payload.session_id, &payload.body, Instant::now()) {
                record(
                    requested,
                    Some(&payload),
                    &source,
                    "skipped",
                    Some("same error already pushed for this session (10 min quiet window)"),
                    None,
                );
                return;
            }
        }
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
        // No detail is a normal outcome, not a failed operation.
        match detail {
            Some(detail) => format!(": {detail}"),
            None => String::new(),
        }
    ));
    let Some(dir) = dirs::home_dir().map(|h| h.join(".jcode").join("logs")) else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(&dir) {
        crate::logging::warn(&format!(
            "cannot create mobile push history directory: {error}"
        ));
        return;
    }
    let path = dir.join("mobile-push.jsonl");
    // Keep the history bounded: rotate once past 5 MB.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024)
        && let Err(error) = std::fs::rename(&path, dir.join("mobile-push.1.jsonl"))
    {
        crate::logging::warn(&format!("cannot rotate mobile push history: {error}"));
        return;
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{line}") {
                crate::logging::warn(&format!("cannot write mobile push history: {error}"));
            }
        }
        Err(error) => crate::logging::warn(&format!("cannot open mobile push history: {error}")),
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
        anyhow::anyhow!(
            "Sign in first with `jcode account login` (or `/login jcode`), then retry /mobile."
        )
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
            let body = match result {
                Ok(body) => body,
                Err(error) => format!("unable to read rejection body: {error}"),
            };
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
    #[test]
    fn resumed_turn_cancels_pending_failure_push() {
        let token = super::queue_pending_error("push-recovered-turn");
        super::clear_cancelled("push-recovered-turn");
        assert!(!super::finish_pending_error("push-recovered-turn", &token));
    }

    #[test]
    fn resuming_one_session_does_not_hide_another_failure() {
        let recovered = super::queue_pending_error("push-isolated-recovered");
        let stopped = super::queue_pending_error("push-isolated-stopped");
        super::clear_cancelled("push-isolated-recovered");
        assert!(!super::finish_pending_error(
            "push-isolated-recovered",
            &recovered
        ));
        assert!(super::finish_pending_error(
            "push-isolated-stopped",
            &stopped
        ));
    }

    #[test]
    fn stopped_turn_keeps_failure_push() {
        let token = super::queue_pending_error("push-stopped-turn");
        assert!(super::finish_pending_error("push-stopped-turn", &token));
    }

    #[test]
    fn stale_error_cannot_remove_new_failure_push() {
        let old = super::queue_pending_error("push-replaced-turn");
        let new = super::queue_pending_error("push-replaced-turn");
        assert!(!super::finish_pending_error("push-replaced-turn", &old));
        super::mark_cancelled("push-replaced-turn");
        assert!(!super::finish_pending_error("push-replaced-turn", &new));
        super::take_cancelled("push-replaced-turn");
    }

    use super::*;

    #[test]
    fn invalid_summary_regex_is_cached_without_panicking() {
        static CACHE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
        assert!(cached_regex(&CACHE, "[").is_none());
        assert!(
            cached_regex(&CACHE, "valid").is_none(),
            "failed initialization is cached"
        );
    }

    #[test]
    fn malformed_error_json_keeps_readable_context() {
        assert_eq!(
            summarize_error("request failed: {not json}: retry later"),
            "request failed: retry later"
        );
        assert_eq!(
            summarize_wake("**Background task stalled** partial"),
            "has been silent for  and may be stuck"
        );
    }

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
                "session_title": "\u{2705} Done",
                "session_name": "Fix login",
                "body": "Done. All tests pass",
                "turn_id": 42
            })
        );
    }

    #[test]
    fn mobile_push_title_leads_with_origin_repo_and_run_time() {
        let labels = PushLabels {
            origin: Some("XPS 14".to_string()),
            repo: Some("jcode-mobile".to_string()),
            elapsed: Some(Duration::from_secs(252)),
        };
        let p = build_payload_with(
            PushKind::TurnDone,
            "session_x",
            Some("Fix login"),
            "Done.",
            Some(7),
            &labels,
        );
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "kind": "turn_done",
                "session_id": "session_x",
                "session_title": "XPS 14 \u{00B7} jcode-mobile \u{00B7} \u{2705} 4m 12s",
                "session_name": "Fix login",
                "body": "Done.",
                "turn_id": 7,
                "origin": "XPS 14",
                "repo": "jcode-mobile",
                "duration_secs": 252
            })
        );
        // Cloud sessions, wakes without a run time, and sessions outside a repo.
        let cloud = PushLabels {
            origin: Some("Cloud".to_string()),
            ..PushLabels::default()
        };
        let p = build_payload_with(PushKind::Wake, "s", Some("Nightly"), "x", None, &cloud);
        assert_eq!(
            p.session_title.as_deref(),
            Some("Cloud \u{00B7} \u{23F0} Update")
        );
        assert_eq!(p.session_name.as_deref(), Some("Nightly"));
        assert_eq!(p.duration_secs, None);
        // A question at the end still reports how long the turn ran.
        let p = build_payload_with(
            PushKind::TurnDone,
            "s",
            Some("T"),
            "Done.\n\nShould I deploy it now?",
            None,
            &labels,
        );
        assert_eq!(p.kind, PushKind::NeedsInput);
        assert_eq!(
            p.session_title.as_deref(),
            Some("XPS 14 \u{00B7} jcode-mobile \u{00B7} \u{2753} Needs input \u{00B7} 4m 12s")
        );
        assert_eq!(p.body, "Should I deploy it now?");
        // Labels never push the title or body past the API's limits.
        let long = "word ".repeat(60);
        let p = build_payload_with(PushKind::TurnError, "s", Some(&long), &long, None, &labels);
        assert_eq!(
            p.session_title.as_deref(),
            Some("XPS 14 \u{00B7} jcode-mobile \u{00B7} \u{274C} Failed \u{00B7} 4m 12s")
        );
        assert!(p.session_name.unwrap().chars().count() <= TITLE_MAX_CHARS);
        assert!(p.body.chars().count() <= BODY_MAX_CHARS);
        assert!(p.body.starts_with("word word"));
    }

    #[test]
    fn mobile_push_failover_prompt_reads_as_plain_words() {
        let prompt = jcode_provider_core::failover::ProviderFailoverPrompt {
            from_provider: "claude".into(),
            from_label: "Anthropic (claude-fox)".into(),
            to_provider: "claude".into(),
            to_label: "Anthropic (claude-otter)".into(),
            target_route: None,
            reason: "503".into(),
            estimated_input_chars: 676703,
            estimated_input_tokens: 169175,
        };
        let p = build_payload(
            PushKind::TurnError,
            "s",
            Some("T"),
            &prompt.to_error_message(),
            None,
        );
        assert_eq!(
            p.body,
            "Anthropic (claude-fox) is unavailable. Open the session to continue on Anthropic (claude-otter)."
        );
    }

    #[test]
    fn mobile_push_repeat_guard_quiets_identical_errors_only() {
        let mut guard = RepeatGuard::default();
        let t0 = Instant::now();
        assert!(!guard.is_repeat("a", "503", t0));
        assert!(guard.is_repeat("a", "503", t0 + Duration::from_secs(60)));
        // Another session, a different error, or any other push in between
        // is news.
        assert!(!guard.is_repeat("b", "503", t0));
        assert!(!guard.is_repeat("a", "401", t0 + Duration::from_secs(61)));
        assert!(guard.is_repeat("a", "401", t0 + Duration::from_secs(62)));
        guard.clear("a");
        assert!(!guard.is_repeat("a", "401", t0 + Duration::from_secs(63)));
        // The quiet window ends on its own.
        assert!(!guard.is_repeat(
            "a",
            "401",
            t0 + Duration::from_secs(64) + REPEAT_ERROR_QUIET
        ));
    }

    #[test]
    fn mobile_push_machine_and_repo_labels() {
        assert_eq!(
            machine_label(Some("XPS 14 DA14260\n"), Some("xps14")),
            "XPS 14"
        );
        assert_eq!(machine_label(Some("MacBook Pro"), None), "MacBook Pro");
        // A product name that is only a SKU, or a board placeholder, says
        // nothing: use the hostname.
        assert_eq!(
            machine_label(Some("20XW00FPUS"), Some("carbon.lan\n")),
            "carbon"
        );
        assert_eq!(
            machine_label(Some("System Product Name"), Some("tower")),
            "tower"
        );
        assert_eq!(machine_label(None, Some("localhost")), "Local");
        assert_eq!(machine_label(None, None), "Local");

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("my-repo");
        let nested = repo.join("crates").join("core");
        std::fs::create_dir_all(&nested).unwrap();
        // No repository above it: the directory names itself.
        assert_eq!(repo_label(&nested).as_deref(), Some("core"));
        std::fs::create_dir(repo.join(".git")).unwrap();
        assert_eq!(repo_label(&nested).as_deref(), Some("my-repo"));
        assert_eq!(repo_label(&repo).as_deref(), Some("my-repo"));
        assert_eq!(repo_label(std::path::Path::new("/")), None);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(repo_label(&home), None);
        }
    }

    #[test]
    fn mobile_push_payload_defaults_and_truncation() {
        let p = build_payload(PushKind::NeedsInput, "s", Some("  "), "", None);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "needs_input");
        assert_eq!(v["session_title"], "\u{2753} Needs input");
        assert!(v.get("session_name").is_none());
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
        assert_eq!(p.session_title.as_deref(), Some("\u{2705} Done"));
        assert_eq!(p.session_name.as_deref(), Some("Fix login"));
        for (kind, body, marker) in [
            (PushKind::TurnDone, "done", "\u{2705} Done"),
            (PushKind::TurnDone, "Should I push?", "\u{2753} Needs input"),
            (PushKind::TurnError, "boom", "\u{274C} Failed"),
            (PushKind::Wake, "bash completed", "\u{23F0} Update"),
        ] {
            let t = build_payload(kind, "s", Some("T"), body, None)
                .session_title
                .unwrap();
            assert_eq!(t, marker);
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
