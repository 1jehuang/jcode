//! Logging infrastructure for jcode
//!
//! Logs to ~/.jcode/logs/ with automatic rotation

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use chrono::Local;

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

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
        let line = format!("[{}] [{}] {}\n", timestamp, level, message);
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
    let msg = format!("TOOL[{}] input={} output={}", name,
        truncate(input, 200), truncate(output, 500));
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
