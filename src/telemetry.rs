use crate::storage;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const TELEMETRY_ENDPOINT: &str = "https://jcode-telemetry.jeremyhuang55555.workers.dev/v1/event";

static SESSION_STATE: Mutex<Option<SessionTelemetry>> = Mutex::new(None);

static ERROR_PROVIDER_TIMEOUT: AtomicU32 = AtomicU32::new(0);
static ERROR_AUTH_FAILED: AtomicU32 = AtomicU32::new(0);
static ERROR_TOOL_ERROR: AtomicU32 = AtomicU32::new(0);
static ERROR_MCP_ERROR: AtomicU32 = AtomicU32::new(0);
static ERROR_RATE_LIMITED: AtomicU32 = AtomicU32::new(0);
static PROVIDER_SWITCHES: AtomicU32 = AtomicU32::new(0);
static MODEL_SWITCHES: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallEvent {
    id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionEndEvent {
    id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    provider_start: String,
    provider_end: String,
    model_start: String,
    model_end: String,
    provider_switches: u32,
    model_switches: u32,
    duration_mins: u64,
    turns: u32,
    errors: ErrorCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ErrorCounts {
    provider_timeout: u32,
    auth_failed: u32,
    tool_error: u32,
    mcp_error: u32,
    rate_limited: u32,
}

struct SessionTelemetry {
    started_at: Instant,
    provider_start: String,
    model_start: String,
    turns: u32,
}

pub fn is_enabled() -> bool {
    if std::env::var("JCODE_NO_TELEMETRY").is_ok() || std::env::var("DO_NOT_TRACK").is_ok() {
        return false;
    }
    if let Ok(dir) = storage::jcode_dir() {
        if dir.join("no_telemetry").exists() {
            return false;
        }
    }
    true
}

fn telemetry_id_path() -> Option<PathBuf> {
    storage::jcode_dir().ok().map(|d| d.join("telemetry_id"))
}

fn get_or_create_id() -> Option<String> {
    let path = telemetry_id_path()?;
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return Some(id);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Some(id)
}

fn is_first_run() -> bool {
    telemetry_id_path().map(|p| !p.exists()).unwrap_or(false)
}

fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn fire_and_forget(payload: serde_json::Value) {
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build();
        let client = match client {
            Ok(c) => c,
            Err(_) => return,
        };
        let _ = client.post(TELEMETRY_ENDPOINT).json(&payload).send();
    });
}

pub fn record_install_if_first_run() {
    if !is_enabled() {
        return;
    }
    if !is_first_run() {
        return;
    }
    let id = match get_or_create_id() {
        Some(id) => id,
        None => return,
    };
    let event = InstallEvent {
        id,
        event: "install",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        fire_and_forget(payload);
    }
    show_first_run_notice();
}

pub fn begin_session(provider: &str, model: &str) {
    if !is_enabled() {
        return;
    }
    let _ = get_or_create_id();
    let state = SessionTelemetry {
        started_at: Instant::now(),
        provider_start: provider.to_string(),
        model_start: model.to_string(),
        turns: 0,
    };
    if let Ok(mut guard) = SESSION_STATE.lock() {
        *guard = Some(state);
    }
    ERROR_PROVIDER_TIMEOUT.store(0, Ordering::Relaxed);
    ERROR_AUTH_FAILED.store(0, Ordering::Relaxed);
    ERROR_TOOL_ERROR.store(0, Ordering::Relaxed);
    ERROR_MCP_ERROR.store(0, Ordering::Relaxed);
    ERROR_RATE_LIMITED.store(0, Ordering::Relaxed);
    PROVIDER_SWITCHES.store(0, Ordering::Relaxed);
    MODEL_SWITCHES.store(0, Ordering::Relaxed);
}

pub fn record_turn() {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            state.turns += 1;
        }
    }
}

pub fn record_error(category: ErrorCategory) {
    match category {
        ErrorCategory::ProviderTimeout => {
            ERROR_PROVIDER_TIMEOUT.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::AuthFailed => {
            ERROR_AUTH_FAILED.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::ToolError => {
            ERROR_TOOL_ERROR.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::McpError => {
            ERROR_MCP_ERROR.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::RateLimited => {
            ERROR_RATE_LIMITED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn record_provider_switch() {
    PROVIDER_SWITCHES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_model_switch() {
    MODEL_SWITCHES.fetch_add(1, Ordering::Relaxed);
}

pub fn end_session(provider_end: &str, model_end: &str) {
    if !is_enabled() {
        return;
    }
    let id = match get_or_create_id() {
        Some(id) => id,
        None => return,
    };
    let state = {
        let mut guard = match SESSION_STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match guard.take() {
            Some(s) => s,
            None => return,
        }
    };
    let duration = state.started_at.elapsed();
    let event = SessionEndEvent {
        id,
        event: "session_end",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        provider_start: state.provider_start,
        provider_end: provider_end.to_string(),
        model_start: state.model_start,
        model_end: model_end.to_string(),
        provider_switches: PROVIDER_SWITCHES.load(Ordering::Relaxed),
        model_switches: MODEL_SWITCHES.load(Ordering::Relaxed),
        duration_mins: duration.as_secs() / 60,
        turns: state.turns,
        errors: ErrorCounts {
            provider_timeout: ERROR_PROVIDER_TIMEOUT.load(Ordering::Relaxed),
            auth_failed: ERROR_AUTH_FAILED.load(Ordering::Relaxed),
            tool_error: ERROR_TOOL_ERROR.load(Ordering::Relaxed),
            mcp_error: ERROR_MCP_ERROR.load(Ordering::Relaxed),
            rate_limited: ERROR_RATE_LIMITED.load(Ordering::Relaxed),
        },
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        fire_and_forget(payload);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ErrorCategory {
    ProviderTimeout,
    AuthFailed,
    ToolError,
    McpError,
    RateLimited,
}

fn show_first_run_notice() {
    eprintln!("\x1b[90m");
    eprintln!("  jcode collects anonymous usage statistics (install count, version, OS,");
    eprintln!("  session duration, provider). No code, filenames, or personal data is sent.");
    eprintln!("  To opt out: export JCODE_NO_TELEMETRY=1");
    eprintln!("  Details: https://github.com/1jehuang/jcode/blob/master/TELEMETRY.md");
    eprintln!("\x1b[0m");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opt_out_env_var() {
        std::env::set_var("JCODE_NO_TELEMETRY", "1");
        assert!(!is_enabled());
        std::env::remove_var("JCODE_NO_TELEMETRY");
    }

    #[test]
    fn test_do_not_track() {
        std::env::set_var("DO_NOT_TRACK", "1");
        assert!(!is_enabled());
        std::env::remove_var("DO_NOT_TRACK");
    }

    #[test]
    fn test_error_counters() {
        ERROR_PROVIDER_TIMEOUT.store(0, Ordering::Relaxed);
        ERROR_TOOL_ERROR.store(0, Ordering::Relaxed);
        record_error(ErrorCategory::ProviderTimeout);
        record_error(ErrorCategory::ProviderTimeout);
        record_error(ErrorCategory::ToolError);
        assert_eq!(ERROR_PROVIDER_TIMEOUT.load(Ordering::Relaxed), 2);
        assert_eq!(ERROR_TOOL_ERROR.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_install_event_serialization() {
        let event = InstallEvent {
            id: "test-uuid".to_string(),
            event: "install",
            version: "0.6.1".to_string(),
            os: "linux",
            arch: "x86_64",
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "install");
        assert_eq!(json["os"], "linux");
    }

    #[test]
    fn test_session_end_event_serialization() {
        let event = SessionEndEvent {
            id: "test-uuid".to_string(),
            event: "session_end",
            version: "0.6.1".to_string(),
            os: "linux",
            arch: "x86_64",
            provider_start: "claude".to_string(),
            provider_end: "openrouter".to_string(),
            model_start: "claude-sonnet-4-20250514".to_string(),
            model_end: "anthropic/claude-sonnet-4".to_string(),
            provider_switches: 1,
            model_switches: 2,
            duration_mins: 45,
            turns: 23,
            errors: ErrorCounts {
                provider_timeout: 2,
                auth_failed: 0,
                tool_error: 1,
                mcp_error: 0,
                rate_limited: 0,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "session_end");
        assert_eq!(json["provider_switches"], 1);
        assert_eq!(json["errors"]["provider_timeout"], 2);
    }
}
