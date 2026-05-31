//! Logging infrastructure for jcode
//!
//! Logs to ~/.jcode/logs/ with automatic rotation
//!
//! Supports thread-local context for server, session, provider, and model info.

use chrono::Local;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);
static TASK_LOG_CONTEXTS: OnceLock<Mutex<HashMap<String, LogContext>>> = OnceLock::new();
static RATE_LIMITS: OnceLock<Mutex<HashMap<String, RateLimitState>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Debug => "DEBUG",
        }
    }

    fn is_enabled(self) -> bool {
        !matches!(self, Self::Debug) || std::env::var("JCODE_TRACE").is_ok()
    }
}

#[derive(Clone, Copy, Debug)]
struct RateLimitState {
    last_emit: Instant,
    suppressed: u64,
}

/// Thread-local logging context
#[derive(Default, Clone)]
pub struct LogContext {
    pub server: Option<String>,
    pub session: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

thread_local! {
    static LOG_CONTEXT: RefCell<LogContext> = RefCell::new(LogContext::default());
}

/// Update just the session in the current context
pub fn set_session(session: &str) {
    if with_task_context_mut(|ctx| {
        ctx.session = Some(session.to_string());
    }) {
        return;
    }

    LOG_CONTEXT.with(|c| {
        c.borrow_mut().session = Some(session.to_string());
    });
}

/// Update just the server in the current context
pub fn set_server(server: &str) {
    if with_task_context_mut(|ctx| {
        ctx.server = Some(server.to_string());
    }) {
        return;
    }

    LOG_CONTEXT.with(|c| {
        c.borrow_mut().server = Some(server.to_string());
    });
}

/// Update provider and model in the current context
pub fn set_provider_info(provider: &str, model: &str) {
    if with_task_context_mut(|ctx| {
        ctx.provider = Some(provider.to_string());
        ctx.model = Some(model.to_string());
    }) {
        return;
    }

    LOG_CONTEXT.with(|c| {
        let mut ctx = c.borrow_mut();
        ctx.provider = Some(provider.to_string());
        ctx.model = Some(model.to_string());
    });
}

/// Get the current context as a prefix string
fn context_prefix() -> String {
    if let Some(task_ctx) = task_context_snapshot() {
        return context_prefix_for(&task_ctx);
    }

    LOG_CONTEXT.with(|c| context_prefix_for(&c.borrow()))
}

fn current_task_id() -> Option<String> {
    tokio::task::try_id().map(|id| id.to_string())
}

fn with_task_context_mut(update: impl FnOnce(&mut LogContext)) -> bool {
    let Some(task_id) = current_task_id() else {
        return false;
    };

    let store = TASK_LOG_CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut contexts) = store.lock() {
        let ctx = contexts.entry(task_id).or_default();
        update(ctx);
        true
    } else {
        false
    }
}

fn task_context_snapshot() -> Option<LogContext> {
    let task_id = current_task_id()?;
    let store = TASK_LOG_CONTEXTS.get()?;
    let contexts = store.lock().ok()?;
    contexts.get(&task_id).cloned()
}

/// Snapshot the current logging context for diagnostics that need stable,
/// session-scoped in-memory keys in addition to the rendered log prefix.
pub fn current_context_snapshot() -> LogContext {
    task_context_snapshot().unwrap_or_else(|| LOG_CONTEXT.with(|c| c.borrow().clone()))
}

fn context_prefix_for(ctx: &LogContext) -> String {
    let mut parts = Vec::new();

    if let Some(ref server) = ctx.server {
        parts.push(format!("srv:{}", server));
    }
    if let Some(ref session) = ctx.session {
        // Truncate session name if too long
        let short = if session.len() > 20 {
            &session[..20]
        } else {
            session
        };
        parts.push(format!("ses:{}", short));
    }
    if let Some(ref provider) = ctx.provider {
        parts.push(format!("prv:{}", provider));
    }
    if let Some(ref model) = ctx.model {
        // Just use first part of model name
        let short = model.split('-').next().unwrap_or(model);
        parts.push(format!("mod:{}", short));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("[{}] ", parts.join("|"))
    }
}

pub struct Logger {
    file: File,
}

fn log_dir() -> Option<PathBuf> {
    jcode_storage::logs_dir().ok()
}

impl Logger {
    fn new() -> Option<Self> {
        let log_dir = log_dir()?;
        jcode_storage::ensure_dir(&log_dir).ok()?;

        // Use date-based log file
        let date = Local::now().format("%Y-%m-%d");
        let path = log_dir.join(format!("jcode-{}.log", date));

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;

        Some(Self { file })
    }

    fn write(&mut self, level: &str, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let ctx = context_prefix();
        let line = format!("[{}] [{}] {}{}\n", timestamp, level, ctx, message);
        if let Err(err) = self.file.write_all(line.as_bytes()) {
            eprintln!("jcode logger write failed: {err}");
            return;
        }
        if let Err(err) = self.file.flush() {
            eprintln!("jcode logger flush failed: {err}");
        }
    }
}

/// Initialize the logger (call once at startup)
pub fn init() {
    let mut guard = match LOGGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        *guard = Logger::new();
    }
}

/// Log an info message
#[expect(
    clippy::collapsible_if,
    reason = "Logger lock + optional logger branching is intentionally straightforward"
)]
pub fn info(message: &str) {
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("INFO", message);
        }
    }
}

/// Log an error message
#[expect(
    clippy::collapsible_if,
    reason = "Logger lock + optional logger branching is intentionally straightforward"
)]
pub fn error(message: &str) {
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("ERROR", message);
        }
    }
}

/// Log a warning message
#[expect(
    clippy::collapsible_if,
    reason = "Logger lock + optional logger branching is intentionally straightforward"
)]
pub fn warn(message: &str) {
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("WARN", message);
        }
    }
}

/// Log a debug message (only if JCODE_TRACE is set)
#[expect(
    clippy::collapsible_if,
    reason = "Debug logging keeps env gating and logger access explicit"
)]
pub fn debug(message: &str) {
    if std::env::var("JCODE_TRACE").is_ok() {
        if let Ok(mut guard) = LOGGER.lock() {
            if let Some(logger) = guard.as_mut() {
                logger.write("DEBUG", message);
            }
        }
    }
}

pub fn event_info<K, V, I>(event_name: &str, fields: I)
where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    event(LogLevel::Info, event_name, fields);
}

pub fn event_warn<K, V, I>(event_name: &str, fields: I)
where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    event(LogLevel::Warn, event_name, fields);
}

pub fn event_error<K, V, I>(event_name: &str, fields: I)
where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    event(LogLevel::Error, event_name, fields);
}

pub fn event_debug<K, V, I>(event_name: &str, fields: I)
where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    event(LogLevel::Debug, event_name, fields);
}

pub fn event<K, V, I>(level: LogLevel, event: &str, fields: I)
where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    if !level.is_enabled() {
        return;
    }
    write_level(level, &format_structured_event(event, fields));
}

pub fn event_rate_limited<K, V, I>(
    level: LogLevel,
    rate_key: &str,
    min_interval: Duration,
    event: &str,
    fields: I,
) where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    if !level.is_enabled() {
        return;
    }

    let now = Instant::now();
    let store = RATE_LIMITS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut suppressed = 0;
    let should_emit = match store.lock() {
        Ok(mut guard) => match guard.get_mut(rate_key) {
            Some(state) if now.duration_since(state.last_emit) < min_interval => {
                state.suppressed = state.suppressed.saturating_add(1);
                false
            }
            Some(state) => {
                suppressed = state.suppressed;
                state.suppressed = 0;
                state.last_emit = now;
                true
            }
            None => {
                guard.insert(
                    rate_key.to_string(),
                    RateLimitState {
                        last_emit: now,
                        suppressed: 0,
                    },
                );
                true
            }
        },
        Err(_) => true,
    };

    if !should_emit {
        return;
    }

    let mut fields: Vec<(String, String)> = fields
        .into_iter()
        .map(|(key, value)| (key.as_ref().to_string(), value.to_string()))
        .collect();
    if suppressed > 0 {
        fields.push(("suppressed".to_string(), suppressed.to_string()));
    }
    write_level(level, &format_structured_event(event, fields));
}

fn write_level(level: LogLevel, message: &str) {
    if let Ok(mut guard) = LOGGER.lock()
        && let Some(logger) = guard.as_mut()
    {
        logger.write(level.as_str(), message);
    }
}

fn format_structured_event<K, V, I>(event: &str, fields: I) -> String
where
    K: AsRef<str>,
    V: ToString,
    I: IntoIterator<Item = (K, V)>,
{
    let event = sanitize_log_value(event);
    let mut ordered = BTreeMap::new();
    for (key, value) in fields {
        let raw_key = key.as_ref();
        let key = sanitize_log_key(raw_key);
        let value = redact_auth_field(raw_key, &value.to_string());
        ordered.insert(key, value);
    }

    if structured_json_enabled() {
        let mut object = serde_json::Map::new();
        object.insert("event".to_string(), serde_json::Value::String(event));
        for (key, value) in ordered {
            object.insert(key, serde_json::Value::String(value));
        }
        return format!("EVENT_JSON {}", serde_json::Value::Object(object));
    }

    let mut parts = Vec::with_capacity(ordered.len() + 1);
    parts.push(format!("event={}", format_log_field_value(&event)));
    parts.extend(
        ordered
            .into_iter()
            .map(|(key, value)| format!("{}={}", key, format_log_field_value(&value))),
    );
    format!("EVENT {}", parts.join(" "))
}

fn structured_json_enabled() -> bool {
    matches!(
        std::env::var("JCODE_LOG_JSON").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn sanitize_log_key(key: &str) -> String {
    let sanitized: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "field".to_string()
    } else {
        truncate(&sanitized, 80)
    }
}

fn format_log_field_value(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '=' || c == '"' || c == '\'')
    {
        serde_json::to_string(value).unwrap_or_else(|_| "\"<unserializable>\"".to_string())
    } else {
        value.to_string()
    }
}

/// Log a structured auth event with conservative redaction.
///
/// Callers should pass only non-secret metadata. This function still redacts any
/// field whose key looks credential-like so accidental tokens/keys do not land in
/// logs.
pub fn auth_event(event: &str, provider: &str, fields: &[(&str, &str)]) {
    let mut parts = vec![
        format!("event={}", sanitize_log_value(event)),
        format!("provider={}", sanitize_log_value(provider)),
    ];
    for (key, value) in fields {
        parts.push(format!(
            "{}={}",
            sanitize_log_value(key),
            redact_auth_field(key, value)
        ));
    }
    let msg = format!("AUTH {}", parts.join(" "));
    if let Ok(mut guard) = LOGGER.lock()
        && let Some(logger) = guard.as_mut()
    {
        logger.write("AUTH", &msg);
    }
}

/// Log a tool call
#[expect(
    clippy::collapsible_if,
    reason = "Logger lock + optional logger branching is intentionally straightforward"
)]
pub fn tool_call(name: &str, input: &str, output: &str) {
    let msg = format!(
        "TOOL[{}] input={} output={}",
        name,
        truncate(input, 200),
        truncate(output, 500)
    );
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("TOOL", &msg);
        }
    }
}

/// Log a crash/panic for auto-debug
#[expect(
    clippy::collapsible_if,
    reason = "Logger lock + optional logger branching is intentionally straightforward"
)]
pub fn crash(error: &str, context: &str) {
    let msg = format!("CRASH: {} | Context: {}", error, context);
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("CRASH", &msg);
        }
    }
}

/// Get the session ID from the current logging context (thread-local or task-local).
pub fn current_session() -> Option<String> {
    if let Some(ctx) = task_context_snapshot() {
        return ctx.session;
    }
    LOG_CONTEXT.with(|c| c.borrow().session.clone())
}

/// Get path to today's log file
pub fn log_path() -> Option<PathBuf> {
    let log_dir = log_dir()?;
    let date = Local::now().format("%Y-%m-%d");
    Some(log_dir.join(format!("jcode-{}.log", date)))
}

/// Clean up old logs (keep last 7 days)
pub fn cleanup_old_logs() {
    if let Some(log_dir) = log_dir()
        && let Ok(entries) = fs::read_dir(&log_dir)
    {
        let cutoff = Local::now() - chrono::Duration::days(7);
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata()
                && let Ok(modified) = metadata.modified()
            {
                let modified: chrono::DateTime<Local> = modified.into();
                if modified < cutoff
                    && let Err(err) = fs::remove_file(entry.path())
                {
                    eprintln!("jcode logger cleanup failed: {err}");
                }
            }
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", jcode_core::util::truncate_str(s, max_len))
    } else {
        s.to_string()
    }
}

fn redact_auth_field(key: &str, value: &str) -> String {
    let key = key.to_ascii_lowercase();
    if key.contains("token")
        || key.contains("secret")
        || key.contains("key")
        || key.contains("credential")
        || key.contains("callback")
        || key.contains("authorization")
        || key.contains("auth_code")
        || key.contains("oauth_code")
        || key.contains("code_verifier")
        || key.contains("code_challenge")
    {
        return "<redacted>".to_string();
    }
    sanitize_log_value(value)
}

fn sanitize_log_value(value: &str) -> String {
    let value = value.replace(['\n', '\r', '\t'], " ");
    let value = redact_url_queries(&value);
    truncate(&value, 160)
}

fn redact_url_queries(value: &str) -> String {
    value
        .split(' ')
        .map(|word| {
            if (word.starts_with("http://") || word.starts_with("https://")) && word.contains('?') {
                let (base, _) = word.split_once('?').unwrap_or((word, ""));
                format!("{}?<redacted>", base)
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_log_redacts_secret_like_fields() {
        assert_eq!(redact_auth_field("api_key", "sk-secret"), "<redacted>");
        assert_eq!(
            redact_auth_field("callback_url", "https://example.com/?code=secret"),
            "<redacted>"
        );
    }

    #[test]
    fn auth_log_sanitizes_urls_and_control_characters() {
        assert_eq!(
            sanitize_log_value("failed\nhttps://login.example.com/cb?code=secret&state=abc"),
            "failed https://login.example.com/cb?<redacted>"
        );
    }

    #[test]
    fn structured_event_orders_and_redacts_fields() {
        let line = format_structured_event(
            "server_request",
            vec![
                ("z", "last"),
                ("api_key", "sk-secret"),
                ("a field", "hello world"),
            ],
        );
        assert_eq!(
            line,
            "EVENT event=server_request a_field=\"hello world\" api_key=<redacted> z=last"
        );
    }

    #[test]
    fn structured_event_redacts_url_queries() {
        let line = format_structured_event(
            "callback",
            vec![("url", "https://example.test/cb?code=secret&state=abc")],
        );
        assert_eq!(
            line,
            "EVENT event=callback url=https://example.test/cb?<redacted>"
        );
    }

    #[test]
    fn structured_event_keeps_non_secret_code_fields() {
        let line = format_structured_event("tool_done", vec![("exit_code", "127")]);
        assert_eq!(line, "EVENT event=tool_done exit_code=127");
    }
}
