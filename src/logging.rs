//! Logging infrastructure for jcode
//!
//! Logs to ~/.jcode/logs/ with automatic rotation
//!
//! Supports thread-local context for server, session, provider, and model info.

#![allow(dead_code)]

use chrono::Local;
use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

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

/// Set the logging context for the current thread
pub fn set_context(ctx: LogContext) {
    LOG_CONTEXT.with(|c| {
        *c.borrow_mut() = ctx;
    });
}

/// Update just the session in the current context
pub fn set_session(session: &str) {
    LOG_CONTEXT.with(|c| {
        c.borrow_mut().session = Some(session.to_string());
    });
}

/// Update just the server in the current context
pub fn set_server(server: &str) {
    LOG_CONTEXT.with(|c| {
        c.borrow_mut().server = Some(server.to_string());
    });
}

/// Update provider and model in the current context
pub fn set_provider_info(provider: &str, model: &str) {
    LOG_CONTEXT.with(|c| {
        let mut ctx = c.borrow_mut();
        ctx.provider = Some(provider.to_string());
        ctx.model = Some(model.to_string());
    });
}

/// Clear the logging context for the current thread
pub fn clear_context() {
    LOG_CONTEXT.with(|c| {
        *c.borrow_mut() = LogContext::default();
    });
}

/// Get the current context as a prefix string
fn context_prefix() -> String {
    LOG_CONTEXT.with(|c| {
        let ctx = c.borrow();
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
    })
}

pub struct Logger {
    file: File,
    path: PathBuf,
}

impl Logger {
    fn new() -> Option<Self> {
        let log_dir = dirs::home_dir()?.join(".jcode").join("logs");
        fs::create_dir_all(&log_dir).ok()?;

        // Use date-based log file
        let date = Local::now().format("%Y-%m-%d");
        let path = log_dir.join(format!("jcode-{}.log", date));

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;

        Some(Self { file, path })
    }

    fn write(&mut self, level: &str, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let ctx = context_prefix();
        let line = format!("[{}] [{}] {}{}\n", timestamp, level, ctx, message);
        let _ = self.file.write_all(line.as_bytes());
        let _ = self.file.flush();
    }
}

/// Initialize the logger (call once at startup)
pub fn init() {
    let mut guard = LOGGER.lock().unwrap();
    if guard.is_none() {
        *guard = Logger::new();
    }
}

/// Log an info message
pub fn info(message: &str) {
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("INFO", message);
        }
    }
}

/// Log an error message
pub fn error(message: &str) {
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("ERROR", message);
        }
    }
}

/// Log a debug message (only if JCODE_TRACE is set)
pub fn debug(message: &str) {
    if std::env::var("JCODE_TRACE").is_ok() {
        if let Ok(mut guard) = LOGGER.lock() {
            if let Some(logger) = guard.as_mut() {
                logger.write("DEBUG", message);
            }
        }
    }
}

/// Log a tool call
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
pub fn crash(error: &str, context: &str) {
    let msg = format!("CRASH: {} | Context: {}", error, context);
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.as_mut() {
            logger.write("CRASH", &msg);
        }
    }
}

/// Get path to today's log file
pub fn log_path() -> Option<PathBuf> {
    let log_dir = dirs::home_dir()?.join(".jcode").join("logs");
    let date = Local::now().format("%Y-%m-%d");
    Some(log_dir.join(format!("jcode-{}.log", date)))
}

/// Clean up old logs (keep last 7 days)
pub fn cleanup_old_logs() {
    if let Some(log_dir) = dirs::home_dir().map(|h| h.join(".jcode").join("logs")) {
        if let Ok(entries) = fs::read_dir(&log_dir) {
            let cutoff = Local::now() - chrono::Duration::days(7);
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let modified: chrono::DateTime<Local> = modified.into();
                        if modified < cutoff {
                            let _ = fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    }
}
