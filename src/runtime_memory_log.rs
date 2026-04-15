use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_SERVER_INTERVAL_SECS: u64 = 5 * 60;
const MIN_SERVER_INTERVAL_SECS: u64 = 30;
const MAX_SERVER_LOG_FILES: usize = 90;
const SERVER_LOG_FILE_PREFIX: &str = "server-runtime-memory-";
const SERVER_LOG_FILE_SUFFIX: &str = ".jsonl";

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemorySample {
    pub schema_version: u32,
    pub timestamp: String,
    pub timestamp_ms: i64,
    pub source: String,
    pub server: ServerRuntimeMemoryServer,
    pub process: crate::process_memory::ProcessMemorySnapshot,
    pub clients: ServerRuntimeMemoryClients,
    pub sessions: ServerRuntimeMemorySessions,
    pub background: ServerRuntimeMemoryBackground,
    pub embeddings: ServerRuntimeMemoryEmbeddings,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemoryServer {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub version: String,
    pub git_hash: String,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemoryClients {
    pub connected_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemoryBackground {
    pub task_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemoryEmbeddings {
    pub model_available: bool,
    #[serde(flatten)]
    pub stats: crate::embedding::EmbedderStats,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ServerRuntimeMemorySessions {
    pub live_count: usize,
    pub sampled_count: usize,
    pub contended_count: usize,
    pub memory_enabled_session_count: usize,
    pub total_message_count: u64,
    pub total_provider_cache_message_count: u64,
    pub total_json_bytes: u64,
    pub total_payload_text_bytes: u64,
    pub total_provider_cache_json_bytes: u64,
    pub total_tool_result_bytes: u64,
    pub total_provider_cache_tool_result_bytes: u64,
    pub total_large_blob_bytes: u64,
    pub total_provider_cache_large_blob_bytes: u64,
    pub top_by_json_bytes: Vec<ServerRuntimeMemoryTopSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemoryTopSession {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub memory_enabled: bool,
    pub message_count: u64,
    pub provider_cache_message_count: u64,
    pub json_bytes: u64,
    pub payload_text_bytes: u64,
    pub provider_cache_json_bytes: u64,
    pub tool_result_bytes: u64,
    pub provider_cache_tool_result_bytes: u64,
    pub large_blob_bytes: u64,
    pub provider_cache_large_blob_bytes: u64,
}

pub fn server_logging_enabled() -> bool {
    match std::env::var("JCODE_RUNTIME_MEMORY_LOG") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

pub fn server_logging_interval() -> Duration {
    let secs = std::env::var("JCODE_RUNTIME_MEMORY_LOG_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= MIN_SERVER_INTERVAL_SECS)
        .unwrap_or(DEFAULT_SERVER_INTERVAL_SECS);
    Duration::from_secs(secs)
}

pub fn server_logs_dir() -> Result<PathBuf> {
    Ok(crate::storage::logs_dir()?.join("memory"))
}

pub fn current_server_log_path() -> Result<PathBuf> {
    server_log_path_for(Utc::now())
}

pub fn append_server_sample(sample: &ServerRuntimeMemorySample) -> Result<PathBuf> {
    let path = current_server_log_path()?;
    crate::storage::append_json_line_fast(&path, sample)?;
    Ok(path)
}

pub fn prune_old_server_logs() -> Result<usize> {
    let dir = server_logs_dir()?;
    if !dir.exists() {
        return Ok(0);
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_server_log_file(path))
        .collect();
    files.sort();

    if files.len() <= MAX_SERVER_LOG_FILES {
        return Ok(0);
    }

    let remove_count = files.len() - MAX_SERVER_LOG_FILES;
    let mut removed = 0;
    for path in files.into_iter().take(remove_count) {
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn server_log_path_for(now: chrono::DateTime<Utc>) -> Result<PathBuf> {
    let dir = server_logs_dir()?;
    let date = now.format("%Y-%m-%d");
    Ok(dir.join(format!("{SERVER_LOG_FILE_PREFIX}{date}{SERVER_LOG_FILE_SUFFIX}")))
}

fn is_server_log_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| {
            name.starts_with(SERVER_LOG_FILE_PREFIX) && name.ends_with(SERVER_LOG_FILE_SUFFIX)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_logging_enabled_defaults_on_and_respects_falsey_env() {
        let _guard = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_RUNTIME_MEMORY_LOG");

        crate::env::remove_var("JCODE_RUNTIME_MEMORY_LOG");
        assert!(server_logging_enabled());

        crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", "0");
        assert!(!server_logging_enabled());

        crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", "false");
        assert!(!server_logging_enabled());

        crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", "1");
        assert!(server_logging_enabled());

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", prev);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_MEMORY_LOG");
        }
    }

    #[test]
    fn append_server_sample_writes_jsonl_under_memory_logs_dir() {
        let _guard = crate::storage::lock_test_env();
        let prev_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().expect("create temp dir");
        crate::env::set_var("JCODE_HOME", temp.path());

        let sample = ServerRuntimeMemorySample {
            schema_version: 1,
            timestamp: Utc::now().to_rfc3339(),
            timestamp_ms: Utc::now().timestamp_millis(),
            source: "test".to_string(),
            server: ServerRuntimeMemoryServer {
                id: "server_test".to_string(),
                name: "test".to_string(),
                icon: "🧪".to_string(),
                version: "v0".to_string(),
                git_hash: "deadbeef".to_string(),
                uptime_secs: 1,
            },
            process: crate::process_memory::ProcessMemorySnapshot::default(),
            clients: ServerRuntimeMemoryClients { connected_count: 0 },
            sessions: ServerRuntimeMemorySessions::default(),
            background: ServerRuntimeMemoryBackground { task_count: 0 },
            embeddings: ServerRuntimeMemoryEmbeddings {
                model_available: false,
                stats: crate::embedding::stats(),
            },
        };

        let path = append_server_sample(&sample).expect("append server sample");
        assert!(path.exists(), "log path should exist: {}", path.display());

        let content = std::fs::read_to_string(&path).expect("read log file");
        let line = content.lines().last().expect("jsonl line");
        let parsed: serde_json::Value = serde_json::from_str(line).expect("parse json line");
        assert_eq!(parsed["source"], "test");
        assert_eq!(parsed["server"]["id"], "server_test");

        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
