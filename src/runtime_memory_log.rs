use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const DEFAULT_PROCESS_INTERVAL_SECS: u64 = 60;
const MIN_PROCESS_INTERVAL_SECS: u64 = 15;
const DEFAULT_ATTRIBUTION_INTERVAL_SECS: u64 = 15 * 60;
const MIN_ATTRIBUTION_INTERVAL_SECS: u64 = 60;
const DEFAULT_ATTRIBUTION_MIN_SPACING_SECS: u64 = 30;
const MIN_ATTRIBUTION_MIN_SPACING_SECS: u64 = 5;
const DEFAULT_EVENT_PROCESS_MIN_SPACING_SECS: u64 = 5;
const MIN_EVENT_PROCESS_MIN_SPACING_SECS: u64 = 1;
const DEFAULT_PSS_DELTA_THRESHOLD_MB: u64 = 16;
const DEFAULT_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB: u64 = 4;
const MAX_SERVER_LOG_FILES: usize = 90;
const SERVER_LOG_FILE_PREFIX: &str = "server-runtime-memory-";
const SERVER_LOG_FILE_SUFFIX: &str = ".jsonl";
const DEFAULT_CLIENT_PROCESS_INTERVAL_SECS: u64 = 5 * 60;
const DEFAULT_CLIENT_ATTRIBUTION_INTERVAL_SECS: u64 = 15 * 60;
const DEFAULT_CLIENT_ATTRIBUTION_MIN_SPACING_SECS: u64 = 30;
const DEFAULT_CLIENT_EVENT_PROCESS_MIN_SPACING_SECS: u64 = 15;
const DEFAULT_CLIENT_PSS_DELTA_THRESHOLD_MB: u64 = 8;
const DEFAULT_CLIENT_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB: u64 = 2;
const MAX_CLIENT_LOG_FILES: usize = 90;
const CLIENT_LOG_FILE_PREFIX: &str = "client-runtime-memory-";
const CLIENT_LOG_FILE_SUFFIX: &str = ".jsonl";
const MAX_PENDING_EVENTS: usize = 64;
const MAX_PENDING_CATEGORIES: usize = 8;

static EVENT_SINK: OnceLock<Mutex<Option<mpsc::UnboundedSender<RuntimeMemoryLogEvent>>>> =
    OnceLock::new();

#[derive(Debug, Clone, Serialize)]
pub struct ServerRuntimeMemorySample {
    pub schema_version: u32,
    pub kind: String,
    pub timestamp: String,
    pub timestamp_ms: i64,
    pub source: String,
    pub trigger: RuntimeMemoryLogTrigger,
    pub sampling: RuntimeMemoryLogSampling,
    pub server: ServerRuntimeMemoryServer,
    pub process: crate::process_memory::ProcessMemorySnapshot,
    pub process_diagnostics: ServerRuntimeMemoryProcessDiagnostics,
    pub clients: ServerRuntimeMemoryClients,
    pub background: ServerRuntimeMemoryBackground,
    pub embeddings: ServerRuntimeMemoryEmbeddings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions: Option<ServerRuntimeMemorySessions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientRuntimeMemorySample {
    pub schema_version: u32,
    pub kind: String,
    pub timestamp: String,
    pub timestamp_ms: i64,
    pub source: String,
    pub trigger: RuntimeMemoryLogTrigger,
    pub sampling: RuntimeMemoryLogSampling,
    pub client: ClientRuntimeMemoryClient,
    pub process: crate::process_memory::ProcessMemorySnapshot,
    pub process_diagnostics: ServerRuntimeMemoryProcessDiagnostics,
    pub totals: ClientRuntimeMemoryTotals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_render: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side_panel_render: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mermaid: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_debug: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientRuntimeMemoryClient {
    pub client_instance_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub is_remote: bool,
    pub is_processing: bool,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ClientRuntimeMemoryTotals {
    pub session_json_bytes: u64,
    pub canonical_transcript_json_bytes: u64,
    pub provider_cache_json_bytes: u64,
    pub provider_messages_json_bytes: u64,
    pub provider_view_json_bytes: u64,
    pub transient_provider_materialization_json_bytes: u64,
    pub display_messages_estimate_bytes: u64,
    pub display_content_bytes: u64,
    pub display_tool_metadata_json_bytes: u64,
    pub display_large_tool_output_bytes: u64,
    pub side_panel_estimate_bytes: u64,
    pub side_panel_content_bytes: u64,
    pub remote_side_pane_images_bytes: u64,
    pub input_text_bytes: u64,
    pub streaming_text_bytes: u64,
    pub thinking_buffer_bytes: u64,
    pub stream_buffered_text_bytes: u64,
    pub streaming_tool_calls_json_bytes: u64,
    pub pasted_contents_bytes: u64,
    pub pending_images_bytes: u64,
    pub remote_state_bytes: u64,
    pub mcp_estimate_bytes: u64,
    pub markdown_cache_estimate_bytes: u64,
    pub ui_render_total_estimate_bytes: u64,
    pub ui_body_cache_estimate_bytes: u64,
    pub ui_full_prep_cache_estimate_bytes: u64,
    pub ui_visible_copy_targets_estimate_bytes: u64,
    pub side_panel_render_total_estimate_bytes: u64,
    pub side_panel_pinned_cache_estimate_bytes: u64,
    pub side_panel_markdown_cache_estimate_bytes: u64,
    pub side_panel_render_cache_estimate_bytes: u64,
    pub mermaid_working_set_estimate_bytes: u64,
    pub mermaid_cache_metadata_estimate_bytes: u64,
    pub visual_debug_frame_estimate_bytes: u64,
    pub total_attributed_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeMemoryLogTrigger {
    pub category: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RuntimeMemoryLogSampling {
    pub forced: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threshold_reasons: Vec<String>,
    pub pending_event_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ServerRuntimeMemoryProcessDiagnostics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocator_active_minus_allocated_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocator_resident_minus_active_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocator_retained_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_minus_allocator_resident_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_minus_allocator_allocated_bytes: Option<i64>,
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

#[derive(Debug, Clone)]
pub struct RuntimeMemoryLogConfig {
    pub process_interval: Duration,
    pub attribution_interval: Duration,
    pub attribution_min_spacing: Duration,
    pub event_process_min_spacing: Duration,
    pub pss_delta_threshold_bytes: u64,
    pub attribution_json_delta_threshold_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct RuntimeMemoryLogEvent {
    pub category: String,
    pub reason: String,
    pub session_id: Option<String>,
    pub detail: Option<String>,
    pub force_attribution: bool,
}

impl RuntimeMemoryLogEvent {
    pub fn new(category: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            reason: reason.into(),
            session_id: None,
            detail: None,
            force_attribution: false,
        }
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn force_attribution(mut self) -> Self {
        self.force_attribution = true;
        self
    }
}

#[derive(Debug)]
pub struct RuntimeMemoryLogController {
    config: RuntimeMemoryLogConfig,
    last_process_sample_at: Option<Instant>,
    last_attribution_at: Option<Instant>,
    last_attribution_pss_bytes: Option<u64>,
    last_attribution_total_json_bytes: Option<u64>,
    pending_events: Vec<RuntimeMemoryLogEvent>,
    pending_attribution_heartbeat: bool,
}

impl RuntimeMemoryLogController {
    pub fn new(config: RuntimeMemoryLogConfig) -> Self {
        Self {
            config,
            last_process_sample_at: None,
            last_attribution_at: None,
            last_attribution_pss_bytes: None,
            last_attribution_total_json_bytes: None,
            pending_events: Vec::new(),
            pending_attribution_heartbeat: false,
        }
    }

    pub fn config(&self) -> &RuntimeMemoryLogConfig {
        &self.config
    }

    pub fn process_heartbeat_due(&self, now: Instant) -> bool {
        self.last_process_sample_at
            .map(|last| now.duration_since(last) >= self.config.process_interval)
            .unwrap_or(true)
    }

    pub fn attribution_heartbeat_due(&self, now: Instant) -> bool {
        self.last_attribution_at
            .map(|last| now.duration_since(last) >= self.config.attribution_interval)
            .unwrap_or(true)
    }

    pub fn should_write_process_for_event(
        &self,
        now: Instant,
        event: &RuntimeMemoryLogEvent,
    ) -> bool {
        event.force_attribution
            || self
                .last_process_sample_at
                .map(|last| {
                    now.saturating_duration_since(last) >= self.config.event_process_min_spacing
                })
                .unwrap_or(true)
    }

    pub fn record_process_sample(&mut self, now: Instant) {
        self.last_process_sample_at = Some(now);
    }

    pub fn defer_event(&mut self, event: RuntimeMemoryLogEvent) {
        if self.pending_events.len() >= MAX_PENDING_EVENTS {
            let overflow = self.pending_events.len() + 1 - MAX_PENDING_EVENTS;
            self.pending_events.drain(0..overflow);
        }
        self.pending_events.push(event);
    }

    pub fn can_write_attribution(&self, now: Instant) -> bool {
        self.last_attribution_at
            .map(|last| now.saturating_duration_since(last) >= self.config.attribution_min_spacing)
            .unwrap_or(true)
    }

    pub fn mark_attribution_heartbeat_pending(&mut self) {
        self.pending_attribution_heartbeat = true;
    }

    pub fn build_sampling_for_process(
        &self,
        event: Option<&RuntimeMemoryLogEvent>,
    ) -> RuntimeMemoryLogSampling {
        let mut pending_categories = pending_categories(&self.pending_events);
        if let Some(event) = event
            && !pending_categories
                .iter()
                .any(|value| value == &event.category)
            && pending_categories.len() < MAX_PENDING_CATEGORIES
        {
            pending_categories.push(event.category.clone());
        }
        RuntimeMemoryLogSampling {
            forced: event.map(|value| value.force_attribution).unwrap_or(false),
            threshold_reasons: Vec::new(),
            pending_event_count: self.pending_events.len(),
            pending_categories,
        }
    }

    pub fn build_sampling_for_attribution(
        &self,
        now: Instant,
        process: &crate::process_memory::ProcessMemorySnapshot,
        event: Option<&RuntimeMemoryLogEvent>,
        heartbeat_reason: Option<&str>,
    ) -> Option<RuntimeMemoryLogSampling> {
        if !self.can_write_attribution(now) {
            return None;
        }

        let mut threshold_reasons = Vec::new();
        let mut forced = false;
        if let Some(event) = event
            && event.force_attribution
        {
            forced = true;
            threshold_reasons.push(format!("event:{}", event.category));
        }
        if !self.pending_events.is_empty() {
            threshold_reasons.push("pending_events".to_string());
            if self
                .pending_events
                .iter()
                .any(|value| value.force_attribution)
            {
                forced = true;
            }
        }
        if self.pending_attribution_heartbeat || heartbeat_reason.is_some() {
            threshold_reasons.push(
                heartbeat_reason
                    .unwrap_or("attribution_heartbeat")
                    .to_string(),
            );
        }
        if self.last_attribution_at.is_none() {
            threshold_reasons.push("initial_attribution".to_string());
        }
        if let Some(pss_reason) = self.pss_delta_reason(process) {
            threshold_reasons.push(pss_reason);
        }

        if threshold_reasons.is_empty() {
            return None;
        }

        Some(RuntimeMemoryLogSampling {
            forced,
            threshold_reasons,
            pending_event_count: self.pending_events.len(),
            pending_categories: pending_categories(&self.pending_events),
        })
    }

    pub fn finalize_attribution_sample(
        &mut self,
        now: Instant,
        sample: &mut ServerRuntimeMemorySample,
    ) {
        let pss_bytes = sample.process.os.as_ref().and_then(|os| os.pss_bytes);
        if let Some(sessions) = sample.sessions.as_ref() {
            self.finalize_attribution_totals(
                now,
                pss_bytes,
                Some(sessions.total_json_bytes),
                &mut sample.sampling.threshold_reasons,
            );
            return;
        }
        self.finalize_attribution_totals(
            now,
            pss_bytes,
            None,
            &mut sample.sampling.threshold_reasons,
        );
    }

    pub fn finalize_attribution_totals(
        &mut self,
        now: Instant,
        pss_bytes: Option<u64>,
        total_json_bytes: Option<u64>,
        threshold_reasons: &mut Vec<String>,
    ) {
        if let Some(total_json_bytes) = total_json_bytes {
            if let Some(last_total_json_bytes) = self.last_attribution_total_json_bytes {
                let delta = total_json_bytes.abs_diff(last_total_json_bytes);
                if delta >= self.config.attribution_json_delta_threshold_bytes {
                    threshold_reasons.push(format!(
                        "attributed_json_delta>= {} MB",
                        bytes_to_mb_string(delta)
                    ));
                }
            }
            self.last_attribution_total_json_bytes = Some(total_json_bytes);
        }
        self.last_attribution_pss_bytes = pss_bytes;
        self.last_attribution_at = Some(now);
        self.pending_events.clear();
        self.pending_attribution_heartbeat = false;
    }

    fn pss_delta_reason(
        &self,
        process: &crate::process_memory::ProcessMemorySnapshot,
    ) -> Option<String> {
        let current_pss = process.os.as_ref()?.pss_bytes?;
        let last_pss = self.last_attribution_pss_bytes?;
        let delta = current_pss.abs_diff(last_pss);
        if delta >= self.config.pss_delta_threshold_bytes {
            Some(format!("pss_delta>= {} MB", bytes_to_mb_string(delta)))
        } else {
            None
        }
    }
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

pub fn server_logging_config() -> RuntimeMemoryLogConfig {
    let legacy_interval_secs = env_u64("JCODE_RUNTIME_MEMORY_LOG_INTERVAL_SECS");
    let process_interval_secs = env_u64("JCODE_RUNTIME_MEMORY_LOG_PROCESS_INTERVAL_SECS")
        .or(legacy_interval_secs)
        .filter(|value| *value >= MIN_PROCESS_INTERVAL_SECS)
        .unwrap_or(DEFAULT_PROCESS_INTERVAL_SECS);
    let attribution_interval_secs = env_u64("JCODE_RUNTIME_MEMORY_LOG_ATTRIBUTION_INTERVAL_SECS")
        .or_else(|| legacy_interval_secs.map(|value| value.saturating_mul(3)))
        .filter(|value| *value >= MIN_ATTRIBUTION_INTERVAL_SECS)
        .unwrap_or(DEFAULT_ATTRIBUTION_INTERVAL_SECS);
    let attribution_min_spacing_secs =
        env_u64("JCODE_RUNTIME_MEMORY_LOG_ATTRIBUTION_MIN_SPACING_SECS")
            .filter(|value| *value >= MIN_ATTRIBUTION_MIN_SPACING_SECS)
            .unwrap_or(DEFAULT_ATTRIBUTION_MIN_SPACING_SECS);
    let event_process_min_spacing_secs =
        env_u64("JCODE_RUNTIME_MEMORY_LOG_EVENT_PROCESS_MIN_SPACING_SECS")
            .filter(|value| *value >= MIN_EVENT_PROCESS_MIN_SPACING_SECS)
            .unwrap_or(DEFAULT_EVENT_PROCESS_MIN_SPACING_SECS);
    let pss_delta_threshold_bytes = env_u64("JCODE_RUNTIME_MEMORY_LOG_PSS_DELTA_THRESHOLD_MB")
        .unwrap_or(DEFAULT_PSS_DELTA_THRESHOLD_MB)
        .saturating_mul(1024 * 1024);
    let attribution_json_delta_threshold_bytes =
        env_u64("JCODE_RUNTIME_MEMORY_LOG_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB")
            .unwrap_or(DEFAULT_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB)
            .saturating_mul(1024 * 1024);

    RuntimeMemoryLogConfig {
        process_interval: Duration::from_secs(process_interval_secs),
        attribution_interval: Duration::from_secs(attribution_interval_secs),
        attribution_min_spacing: Duration::from_secs(attribution_min_spacing_secs),
        event_process_min_spacing: Duration::from_secs(event_process_min_spacing_secs),
        pss_delta_threshold_bytes,
        attribution_json_delta_threshold_bytes,
    }
}

pub fn client_logging_enabled() -> bool {
    match std::env::var("JCODE_CLIENT_RUNTIME_MEMORY_LOG") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => server_logging_enabled(),
    }
}

pub fn client_logging_config() -> RuntimeMemoryLogConfig {
    let process_interval_secs = env_u64("JCODE_CLIENT_RUNTIME_MEMORY_LOG_PROCESS_INTERVAL_SECS")
        .filter(|value| *value >= MIN_PROCESS_INTERVAL_SECS)
        .unwrap_or(DEFAULT_CLIENT_PROCESS_INTERVAL_SECS);
    let attribution_interval_secs =
        env_u64("JCODE_CLIENT_RUNTIME_MEMORY_LOG_ATTRIBUTION_INTERVAL_SECS")
            .filter(|value| *value >= MIN_ATTRIBUTION_INTERVAL_SECS)
            .unwrap_or(DEFAULT_CLIENT_ATTRIBUTION_INTERVAL_SECS);
    let attribution_min_spacing_secs =
        env_u64("JCODE_CLIENT_RUNTIME_MEMORY_LOG_ATTRIBUTION_MIN_SPACING_SECS")
            .filter(|value| *value >= MIN_ATTRIBUTION_MIN_SPACING_SECS)
            .unwrap_or(DEFAULT_CLIENT_ATTRIBUTION_MIN_SPACING_SECS);
    let event_process_min_spacing_secs =
        env_u64("JCODE_CLIENT_RUNTIME_MEMORY_LOG_EVENT_PROCESS_MIN_SPACING_SECS")
            .filter(|value| *value >= MIN_EVENT_PROCESS_MIN_SPACING_SECS)
            .unwrap_or(DEFAULT_CLIENT_EVENT_PROCESS_MIN_SPACING_SECS);
    let pss_delta_threshold_bytes =
        env_u64("JCODE_CLIENT_RUNTIME_MEMORY_LOG_PSS_DELTA_THRESHOLD_MB")
            .unwrap_or(DEFAULT_CLIENT_PSS_DELTA_THRESHOLD_MB)
            .saturating_mul(1024 * 1024);
    let attribution_json_delta_threshold_bytes =
        env_u64("JCODE_CLIENT_RUNTIME_MEMORY_LOG_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB")
            .unwrap_or(DEFAULT_CLIENT_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB)
            .saturating_mul(1024 * 1024);

    RuntimeMemoryLogConfig {
        process_interval: Duration::from_secs(process_interval_secs),
        attribution_interval: Duration::from_secs(attribution_interval_secs),
        attribution_min_spacing: Duration::from_secs(attribution_min_spacing_secs),
        event_process_min_spacing: Duration::from_secs(event_process_min_spacing_secs),
        pss_delta_threshold_bytes,
        attribution_json_delta_threshold_bytes,
    }
}

pub fn install_event_sink(sender: mpsc::UnboundedSender<RuntimeMemoryLogEvent>) {
    if let Ok(mut guard) = event_sink().lock() {
        *guard = Some(sender);
    }
}

pub fn emit_event(event: RuntimeMemoryLogEvent) {
    if let Ok(guard) = event_sink().lock()
        && let Some(sender) = guard.as_ref()
    {
        let _ = sender.send(event);
    }
}

pub fn server_logs_dir() -> Result<PathBuf> {
    Ok(crate::storage::logs_dir()?.join("memory"))
}

pub fn current_server_log_path() -> Result<PathBuf> {
    server_log_path_for(Utc::now())
}

pub fn current_client_log_path() -> Result<PathBuf> {
    client_log_path_for(Utc::now())
}

pub fn append_server_sample(sample: &ServerRuntimeMemorySample) -> Result<PathBuf> {
    let path = current_server_log_path()?;
    crate::storage::append_json_line_fast(&path, sample)?;
    Ok(path)
}

pub fn append_client_sample(sample: &ClientRuntimeMemorySample) -> Result<PathBuf> {
    let path = current_client_log_path()?;
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

pub fn prune_old_client_logs() -> Result<usize> {
    let dir = server_logs_dir()?;
    if !dir.exists() {
        return Ok(0);
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_client_log_file(path))
        .collect();
    files.sort();

    if files.len() <= MAX_CLIENT_LOG_FILES {
        return Ok(0);
    }

    let remove_count = files.len() - MAX_CLIENT_LOG_FILES;
    let mut removed = 0;
    for path in files.into_iter().take(remove_count) {
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn build_process_diagnostics(
    process: &crate::process_memory::ProcessMemorySnapshot,
) -> ServerRuntimeMemoryProcessDiagnostics {
    let allocator_stats = process.allocator.stats.as_ref();
    let rss_bytes = process.rss_bytes;
    let pss_bytes = process.os.as_ref().and_then(|os| os.pss_bytes);
    let allocated_bytes = allocator_stats.and_then(|stats| stats.allocated_bytes);
    let active_bytes = allocator_stats.and_then(|stats| stats.active_bytes);
    let resident_bytes = allocator_stats.and_then(|stats| stats.resident_bytes);
    let retained_bytes = allocator_stats.and_then(|stats| stats.retained_bytes);

    ServerRuntimeMemoryProcessDiagnostics {
        allocator_active_minus_allocated_bytes: delta_i64(active_bytes, allocated_bytes),
        allocator_resident_minus_active_bytes: delta_i64(resident_bytes, active_bytes),
        allocator_retained_bytes: retained_bytes,
        rss_minus_allocator_resident_bytes: delta_i64(rss_bytes, resident_bytes),
        pss_minus_allocator_allocated_bytes: delta_i64(pss_bytes, allocated_bytes),
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse::<u64>().ok()
}

fn event_sink() -> &'static Mutex<Option<mpsc::UnboundedSender<RuntimeMemoryLogEvent>>> {
    EVENT_SINK.get_or_init(|| Mutex::new(None))
}

fn pending_categories(events: &[RuntimeMemoryLogEvent]) -> Vec<String> {
    let mut categories = Vec::new();
    for event in events {
        if categories.iter().any(|value| value == &event.category) {
            continue;
        }
        categories.push(event.category.clone());
        if categories.len() >= MAX_PENDING_CATEGORIES {
            break;
        }
    }
    categories
}

fn delta_i64(left: Option<u64>, right: Option<u64>) -> Option<i64> {
    let left = left? as i128;
    let right = right? as i128;
    let delta = left - right;
    Some(delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
}

fn bytes_to_mb_string(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

fn server_log_path_for(now: chrono::DateTime<Utc>) -> Result<PathBuf> {
    let dir = server_logs_dir()?;
    let date = now.format("%Y-%m-%d");
    Ok(dir.join(format!(
        "{SERVER_LOG_FILE_PREFIX}{date}{SERVER_LOG_FILE_SUFFIX}"
    )))
}

fn client_log_path_for(now: chrono::DateTime<Utc>) -> Result<PathBuf> {
    let dir = server_logs_dir()?;
    let date = now.format("%Y-%m-%d");
    Ok(dir.join(format!(
        "{CLIENT_LOG_FILE_PREFIX}{date}{CLIENT_LOG_FILE_SUFFIX}"
    )))
}

fn is_server_log_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| {
            name.starts_with(SERVER_LOG_FILE_PREFIX) && name.ends_with(SERVER_LOG_FILE_SUFFIX)
        })
        .unwrap_or(false)
}

fn is_client_log_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| {
            name.starts_with(CLIENT_LOG_FILE_PREFIX) && name.ends_with(CLIENT_LOG_FILE_SUFFIX)
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
            schema_version: 2,
            kind: "process".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            timestamp_ms: Utc::now().timestamp_millis(),
            source: "test".to_string(),
            trigger: RuntimeMemoryLogTrigger {
                category: "test".to_string(),
                reason: "unit".to_string(),
                session_id: None,
                detail: None,
            },
            sampling: RuntimeMemoryLogSampling::default(),
            server: ServerRuntimeMemoryServer {
                id: "server_test".to_string(),
                name: "test".to_string(),
                icon: "🧪".to_string(),
                version: "v0".to_string(),
                git_hash: "deadbeef".to_string(),
                uptime_secs: 1,
            },
            process: crate::process_memory::ProcessMemorySnapshot::default(),
            process_diagnostics: ServerRuntimeMemoryProcessDiagnostics::default(),
            clients: ServerRuntimeMemoryClients { connected_count: 0 },
            sessions: None,
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
        assert_eq!(parsed["kind"], "process");

        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn append_client_sample_writes_jsonl_under_memory_logs_dir() {
        let _guard = crate::storage::lock_test_env();
        let prev_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().expect("create temp dir");
        crate::env::set_var("JCODE_HOME", temp.path());

        let sample = ClientRuntimeMemorySample {
            schema_version: 2,
            kind: "process".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            timestamp_ms: Utc::now().timestamp_millis(),
            source: "test".to_string(),
            trigger: RuntimeMemoryLogTrigger {
                category: "test".to_string(),
                reason: "unit".to_string(),
                session_id: Some("session_test".to_string()),
                detail: None,
            },
            sampling: RuntimeMemoryLogSampling::default(),
            client: ClientRuntimeMemoryClient {
                client_instance_id: "client_test".to_string(),
                session_id: "session_test".to_string(),
                remote_session_id: None,
                provider: "mock".to_string(),
                model: "test-model".to_string(),
                is_remote: false,
                is_processing: false,
                uptime_secs: 1,
            },
            process: crate::process_memory::ProcessMemorySnapshot::default(),
            process_diagnostics: ServerRuntimeMemoryProcessDiagnostics::default(),
            totals: ClientRuntimeMemoryTotals::default(),
            session: None,
            ui: None,
            ui_render: None,
            side_panel_render: None,
            markdown: None,
            mermaid: None,
            visual_debug: None,
        };

        let path = append_client_sample(&sample).expect("append client sample");
        assert!(path.starts_with(temp.path()));
        let contents = std::fs::read_to_string(&path).expect("read client log");
        assert!(contents.contains("\"client_test\""));
        assert!(contents.contains("\"session_test\""));

        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn controller_defers_attribution_until_min_spacing() {
        let config = RuntimeMemoryLogConfig {
            process_interval: Duration::from_secs(60),
            attribution_interval: Duration::from_secs(300),
            attribution_min_spacing: Duration::from_secs(30),
            event_process_min_spacing: Duration::from_secs(5),
            pss_delta_threshold_bytes: 16 * 1024 * 1024,
            attribution_json_delta_threshold_bytes: 4 * 1024 * 1024,
        };
        let mut controller = RuntimeMemoryLogController::new(config);
        let now = Instant::now();
        controller.finalize_attribution_sample(
            now,
            &mut ServerRuntimeMemorySample {
                schema_version: 2,
                kind: "attribution".to_string(),
                timestamp: Utc::now().to_rfc3339(),
                timestamp_ms: Utc::now().timestamp_millis(),
                source: "test".to_string(),
                trigger: RuntimeMemoryLogTrigger {
                    category: "startup".to_string(),
                    reason: "unit".to_string(),
                    session_id: None,
                    detail: None,
                },
                sampling: RuntimeMemoryLogSampling::default(),
                server: ServerRuntimeMemoryServer {
                    id: "server_test".to_string(),
                    name: "test".to_string(),
                    icon: "🧪".to_string(),
                    version: "v0".to_string(),
                    git_hash: "deadbeef".to_string(),
                    uptime_secs: 1,
                },
                process: crate::process_memory::ProcessMemorySnapshot::default(),
                process_diagnostics: ServerRuntimeMemoryProcessDiagnostics::default(),
                clients: ServerRuntimeMemoryClients { connected_count: 0 },
                sessions: Some(ServerRuntimeMemorySessions::default()),
                background: ServerRuntimeMemoryBackground { task_count: 0 },
                embeddings: ServerRuntimeMemoryEmbeddings {
                    model_available: false,
                    stats: crate::embedding::stats(),
                },
            },
        );
        let process = crate::process_memory::ProcessMemorySnapshot::default();
        assert!(
            controller
                .build_sampling_for_attribution(
                    now + Duration::from_secs(10),
                    &process,
                    Some(&RuntimeMemoryLogEvent::new("turn_completed", "turn").force_attribution()),
                    None,
                )
                .is_none()
        );
        assert!(
            controller
                .build_sampling_for_attribution(
                    now + Duration::from_secs(31),
                    &process,
                    Some(&RuntimeMemoryLogEvent::new("turn_completed", "turn").force_attribution()),
                    None,
                )
                .is_some()
        );
    }
}
