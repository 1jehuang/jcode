use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hint::black_box;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tokio::sync::broadcast;

pub const HARNESS_EVENT_SCHEMA_VERSION: u16 = 1;
pub const HARNESS_EVENT_REDACTED: &str = "[redacted]";
pub const HARNESS_EVENT_TRUNCATED: &str = "...[truncated]";
pub const DEFAULT_MAX_PAYLOAD_STRING_CHARS: usize = 4096;
pub const HARNESS_EVENT_SSE_CONTENT_TYPE: &str = "text/event-stream";
pub const DEFAULT_HARNESS_EVENT_SSE_RETRY_MS: u64 = 2_000;
const DEFAULT_EVENT_BUS_CAPACITY: usize = 1024;
const HARNESS_EVENT_LOG_DIR: &str = "harness-events";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventKind {
    RunStarted,
    RunCompleted,
    RunFailed,
    SkillSelected,
    MemoryLookupStarted,
    MemoryLookupFinished,
    ToolStarted,
    ToolFinished,
    FileChanged,
    TestStarted,
    TestPassed,
    TestFailed,
    GatePassed,
    GateFailed,
    HumanApprovalRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventPayloadClass {
    SafeMetadata,
    SensitiveMetadata,
    Secret,
    UserContent,
    ArtifactReference,
}

impl HarnessEventPayloadClass {
    pub fn redacts_whole_payload(self) -> bool {
        matches!(self, Self::Secret | Self::UserContent)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HarnessEvent {
    pub schema_version: u16,
    pub event_id: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
    pub level: HarnessEventLevel,
    pub kind: HarnessEventKind,
    pub payload_class: HarnessEventPayloadClass,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventLogSummary {
    pub run_id: String,
    pub path: String,
    pub events: usize,
    pub status: String,
    pub first_timestamp: Option<DateTime<Utc>>,
    pub last_timestamp: Option<DateTime<Utc>>,
    pub duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventReadDiagnostic {
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventReadReport {
    pub path: String,
    pub events: Vec<HarnessEvent>,
    pub diagnostics: Vec<HarnessEventReadDiagnostic>,
    pub partial: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventTimelineItem {
    pub ordinal: usize,
    pub sequence: u64,
    pub event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub elapsed_ms: Option<u128>,
    pub phase: String,
    pub level: HarnessEventLevel,
    pub kind: HarnessEventKind,
    pub status: String,
    pub duration_ms: Option<u128>,
    pub child_count: usize,
    pub failure: bool,
    pub details: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HarnessEventBenchmarkOptions {
    pub events: usize,
}

impl Default for HarnessEventBenchmarkOptions {
    fn default() -> Self {
        Self { events: 10_000 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventBenchmarkMetric {
    pub total_nanos: u128,
    pub micros_per_event: f64,
    pub events_per_second: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventBenchmarkReport {
    pub events: usize,
    pub ndjson_bytes: usize,
    pub read_diagnostics: usize,
    pub publish_no_subscribers: HarnessEventBenchmarkMetric,
    pub ndjson_write_memory: HarnessEventBenchmarkMetric,
    pub ndjson_write_file: HarnessEventBenchmarkMetric,
    pub ndjson_read_report_file: HarnessEventBenchmarkMetric,
    pub timeline_build: HarnessEventBenchmarkMetric,
    pub notes: Vec<String>,
}

impl HarnessEvent {
    pub fn new(
        event_id: impl Into<String>,
        run_id: impl Into<String>,
        timestamp: DateTime<Utc>,
        sequence: u64,
        level: HarnessEventLevel,
        kind: HarnessEventKind,
        payload: Value,
    ) -> Self {
        Self {
            schema_version: HARNESS_EVENT_SCHEMA_VERSION,
            event_id: event_id.into(),
            run_id: run_id.into(),
            session_id: None,
            parent_event_id: None,
            timestamp,
            sequence,
            level,
            kind,
            payload_class: HarnessEventPayloadClass::SafeMetadata,
            payload: redact_harness_event_payload(payload, HarnessEventPayloadClass::SafeMetadata),
        }
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_parent_event_id(mut self, parent_event_id: impl Into<String>) -> Self {
        self.parent_event_id = Some(parent_event_id.into());
        self
    }

    pub fn with_payload_class(mut self, payload_class: HarnessEventPayloadClass) -> Self {
        self.payload_class = payload_class;
        self.payload = redact_harness_event_payload(self.payload, payload_class);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HarnessEventDraft {
    run_id: String,
    session_id: Option<String>,
    parent_event_id: Option<String>,
    level: HarnessEventLevel,
    kind: HarnessEventKind,
    payload_class: HarnessEventPayloadClass,
    payload: Value,
}

impl HarnessEventDraft {
    pub fn new(run_id: impl Into<String>, kind: HarnessEventKind) -> Self {
        Self {
            run_id: run_id.into(),
            session_id: None,
            parent_event_id: None,
            level: HarnessEventLevel::Info,
            kind,
            payload_class: HarnessEventPayloadClass::SafeMetadata,
            payload: json!({}),
        }
    }

    pub fn run_started(run_id: impl Into<String>) -> Self {
        Self::new(run_id, HarnessEventKind::RunStarted)
    }

    pub fn run_completed(run_id: impl Into<String>) -> Self {
        Self::new(run_id, HarnessEventKind::RunCompleted)
    }

    pub fn run_failed(run_id: impl Into<String>) -> Self {
        Self::new(run_id, HarnessEventKind::RunFailed).with_level(HarnessEventLevel::Error)
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_parent_event_id(mut self, parent_event_id: impl Into<String>) -> Self {
        self.parent_event_id = Some(parent_event_id.into());
        self
    }

    pub fn with_level(mut self, level: HarnessEventLevel) -> Self {
        self.level = level;
        self
    }

    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn with_payload_class(mut self, payload_class: HarnessEventPayloadClass) -> Self {
        self.payload_class = payload_class;
        self
    }
}

pub fn redact_harness_event_payload(
    payload: Value,
    payload_class: HarnessEventPayloadClass,
) -> Value {
    if payload_class.redacts_whole_payload() {
        return json!({
            "redacted": true,
            "payload_class": payload_class,
        });
    }

    redact_value(payload)
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_sensitive_payload_key(&key) {
                        (key, Value::String(HARNESS_EVENT_REDACTED.to_string()))
                    } else {
                        (key, redact_value(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_value).collect()),
        Value::String(value) if looks_like_secret_value(&value) => {
            Value::String(HARNESS_EVENT_REDACTED.to_string())
        }
        Value::String(value) => Value::String(truncate_payload_string(&value)),
        other => other,
    }
}

fn is_sensitive_payload_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();

    if is_safe_token_metric_key(&normalized) {
        return false;
    }

    matches!(
        normalized.as_str(),
        "apikey"
            | "authorization"
            | "authtoken"
            | "cookie"
            | "credential"
            | "credentials"
            | "filecontent"
            | "input"
            | "password"
            | "privatekey"
            | "prompt"
            | "rawprompt"
            | "refreshtoken"
            | "secret"
            | "setcookie"
            | "stderr"
            | "stdout"
            | "token"
            | "tooloutput"
            | "usercontent"
    ) || normalized.contains("apikey")
        || normalized.contains("privatekey")
        || normalized.contains("secret")
        || normalized.contains("token")
}

fn is_safe_token_metric_key(normalized_key: &str) -> bool {
    matches!(
        normalized_key,
        "cachecreationinputtokens"
            | "cachereadinputtokens"
            | "completiontokens"
            | "inputtokens"
            | "outputtokens"
            | "prompttokens"
            | "tokencount"
            | "tokensused"
            | "totaltokens"
    )
}

fn looks_like_secret_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("gho_")
        || trimmed.starts_with("github_pat_")
        || trimmed.starts_with("sk-")
        || trimmed.contains("-----BEGIN ")
}

fn truncate_payload_string(value: &str) -> String {
    let mut chars = value.chars();
    let truncated: String = chars
        .by_ref()
        .take(DEFAULT_MAX_PAYLOAD_STRING_CHARS)
        .collect();
    if chars.next().is_none() {
        value.to_string()
    } else {
        format!("{truncated}{HARNESS_EVENT_TRUNCATED}")
    }
}

pub fn default_harness_event_log_dir() -> PathBuf {
    crate::storage::runtime_dir().join(HARNESS_EVENT_LOG_DIR)
}

pub fn harness_event_log_path(run_id: &str) -> PathBuf {
    default_harness_event_log_dir().join(format!("{}.ndjson", sanitize_run_id(run_id)))
}

pub fn write_harness_event_ndjson<W: Write>(
    writer: &mut W,
    event: &HarnessEvent,
) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *writer, event)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn append_harness_event_ndjson(
    path: impl AsRef<Path>,
    event: &HarnessEvent,
) -> anyhow::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    write_harness_event_ndjson(&mut file, event)
}

pub fn render_harness_event_sse(
    event: &HarnessEvent,
    retry_ms: Option<u64>,
) -> anyhow::Result<String> {
    let mut output = Vec::new();
    write_harness_event_sse(&mut output, event, retry_ms)?;
    Ok(String::from_utf8(output)?)
}

pub fn write_harness_event_sse<W: Write>(
    writer: &mut W,
    event: &HarnessEvent,
    retry_ms: Option<u64>,
) -> anyhow::Result<()> {
    writeln!(writer, "id: {}", sanitize_sse_field_value(&event.event_id))?;
    writeln!(writer, "event: {}", harness_event_sse_event_name(event))?;
    if let Some(retry_ms) = retry_ms {
        writeln!(writer, "retry: {}", retry_ms)?;
    }
    let data = serde_json::to_string(event)?;
    for line in data.lines() {
        writeln!(writer, "data: {}", line)?;
    }
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}

pub fn harness_event_sse_event_name(event: &HarnessEvent) -> String {
    event_label(&event.kind)
}

pub fn harness_events_after_last_event_id<'a>(
    events: &'a [HarnessEvent],
    last_event_id: Option<&str>,
) -> &'a [HarnessEvent] {
    let Some(last_event_id) = last_event_id.filter(|id| !id.is_empty()) else {
        return events;
    };

    events
        .iter()
        .position(|event| event.event_id == last_event_id)
        .map(|index| &events[index + 1..])
        .unwrap_or(events)
}

fn sanitize_sse_field_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

pub fn run_harness_event_benchmark(
    options: HarnessEventBenchmarkOptions,
) -> anyhow::Result<HarnessEventBenchmarkReport> {
    let event_count = options.events.max(1);
    let run_id = format!("bench_{}", crate::id::new_id("hevtbench"));
    let bus = HarnessEventBus::with_capacity(1);

    let publish_start = Instant::now();
    let mut events = Vec::with_capacity(event_count);
    for index in 0..event_count {
        let kind = if index == 0 {
            HarnessEventKind::RunStarted
        } else if index + 1 == event_count {
            HarnessEventKind::RunCompleted
        } else {
            HarnessEventKind::ToolFinished
        };
        let payload = json!({
            "status": if matches!(kind, HarnessEventKind::RunStarted) { "started" } else { "ok" },
            "tool": "synthetic-bench",
            "duration_ms": 1,
            "index": index,
        });
        let event = bus.publish(HarnessEventDraft::new(&run_id, kind).with_payload(payload));
        black_box(&event);
        events.push(event);
    }
    let publish_no_subscribers = benchmark_metric(publish_start, event_count);

    let memory_write_start = Instant::now();
    let mut ndjson = Vec::new();
    for event in &events {
        write_harness_event_ndjson(&mut ndjson, event)?;
    }
    black_box(ndjson.len());
    let ndjson_write_memory = benchmark_metric(memory_write_start, event_count);

    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-events-bench-")
        .tempdir()?;
    let path = temp.path().join("bench.ndjson");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    let file_write_start = Instant::now();
    for event in &events {
        write_harness_event_ndjson(&mut file, event)?;
    }
    let ndjson_write_file = benchmark_metric(file_write_start, event_count);
    drop(file);

    let read_start = Instant::now();
    let read_report = read_harness_event_ndjson_report(&path)?;
    black_box(read_report.events.len());
    let ndjson_read_report_file = benchmark_metric(read_start, event_count);

    let timeline_start = Instant::now();
    let timeline = build_harness_event_timeline(&read_report.events);
    black_box(timeline.len());
    let timeline_build = benchmark_metric(timeline_start, event_count);

    Ok(HarnessEventBenchmarkReport {
        events: event_count,
        ndjson_bytes: ndjson.len(),
        read_diagnostics: read_report.diagnostics.len(),
        publish_no_subscribers,
        ndjson_write_memory,
        ndjson_write_file,
        ndjson_read_report_file,
        timeline_build,
        notes: vec![
            "Synthetic single-process baseline; compare on the same machine/profile.".to_string(),
            "No fsync is used; NDJSON writes flush per event like the production sink.".to_string(),
            "Use larger --events values locally before setting regression thresholds.".to_string(),
        ],
    })
}

fn benchmark_metric(start: Instant, events: usize) -> HarnessEventBenchmarkMetric {
    let elapsed = start.elapsed();
    let total_nanos = elapsed.as_nanos();
    let events = events.max(1) as f64;
    let elapsed_secs = elapsed.as_secs_f64();

    HarnessEventBenchmarkMetric {
        total_nanos,
        micros_per_event: total_nanos as f64 / 1_000.0 / events,
        events_per_second: if elapsed_secs > 0.0 {
            events / elapsed_secs
        } else {
            f64::INFINITY
        },
    }
}

pub fn read_harness_event_ndjson_report(
    path: impl AsRef<Path>,
) -> anyhow::Result<HarnessEventReadReport> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).map_err(|err| {
        anyhow::anyhow!(
            "failed to open harness event log {}: {}",
            path.display(),
            err
        )
    })?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut diagnostics = Vec::new();

    for (line_index, line) in reader.lines().enumerate() {
        let line_number = line_index + 1;
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                diagnostics.push(HarnessEventReadDiagnostic {
                    line: line_number,
                    message: format!(
                        "failed to read harness event log {} line {}: {}",
                        path.display(),
                        line_number,
                        err
                    ),
                });
                continue;
            }
        };
        if line.trim().is_empty() {
            diagnostics.push(HarnessEventReadDiagnostic {
                line: line_number,
                message: format!(
                    "invalid harness event NDJSON at {} line {}: blank line",
                    path.display(),
                    line_number
                ),
            });
            continue;
        }
        match serde_json::from_str::<HarnessEvent>(&line) {
            Ok(event) => events.push(event),
            Err(err) => diagnostics.push(HarnessEventReadDiagnostic {
                line: line_number,
                message: format!(
                    "invalid harness event NDJSON at {} line {}: {}",
                    path.display(),
                    line_number,
                    err
                ),
            }),
        }
    }

    Ok(HarnessEventReadReport {
        path: path.display().to_string(),
        events,
        partial: !diagnostics.is_empty(),
        diagnostics,
    })
}

pub fn read_harness_event_ndjson(path: impl AsRef<Path>) -> anyhow::Result<Vec<HarnessEvent>> {
    let report = read_harness_event_ndjson_report(path)?;

    if let Some(first) = report.diagnostics.first() {
        anyhow::bail!("{}", first.message);
    }

    Ok(report.events)
}

pub fn summarize_harness_event_read_report(
    report: &HarnessEventReadReport,
) -> HarnessEventLogSummary {
    let mut summary = summarize_harness_events(&report.path, &report.events);
    if !report.diagnostics.is_empty() {
        if report.events.is_empty() {
            summary.status = "corrupt".to_string();
        } else if summary.status != "failed" {
            summary.status = "partial".to_string();
        }
        summary.error = Some(format_harness_event_read_diagnostics(&report.diagnostics));
    }
    summary
}

fn format_harness_event_read_diagnostics(diagnostics: &[HarnessEventReadDiagnostic]) -> String {
    let mut message = diagnostics
        .iter()
        .take(3)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if diagnostics.len() > 3 {
        message.push_str(&format!(
            "; ... {} more diagnostic(s)",
            diagnostics.len() - 3
        ));
    }
    message
}

pub fn summarize_harness_event_log(
    path: impl AsRef<Path>,
) -> anyhow::Result<HarnessEventLogSummary> {
    let report = read_harness_event_ndjson_report(path)?;
    Ok(summarize_harness_event_read_report(&report))
}

pub fn summarize_harness_events(
    path: impl AsRef<Path>,
    events: &[HarnessEvent],
) -> HarnessEventLogSummary {
    let path = path.as_ref();
    let first_timestamp = events.iter().map(|event| event.timestamp).min();
    let last_timestamp = events.iter().map(|event| event.timestamp).max();
    let run_id = events
        .first()
        .map(|event| event.run_id.clone())
        .unwrap_or_else(|| run_id_from_event_log_path(path));
    let status = infer_harness_event_status(events).to_string();
    let duration_ms = first_timestamp
        .zip(last_timestamp)
        .and_then(|(first, last)| (last - first).num_milliseconds().try_into().ok());

    HarnessEventLogSummary {
        run_id,
        path: path.display().to_string(),
        events: events.len(),
        status,
        first_timestamp,
        last_timestamp,
        duration_ms,
        error: None,
    }
}

pub fn list_harness_event_logs() -> anyhow::Result<Vec<HarnessEventLogSummary>> {
    let dir = default_harness_event_log_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("ndjson") {
            continue;
        }
        match summarize_harness_event_log(&path) {
            Ok(summary) => summaries.push(summary),
            Err(err) => summaries.push(HarnessEventLogSummary {
                run_id: run_id_from_event_log_path(&path),
                path: path.display().to_string(),
                events: 0,
                status: "corrupt".to_string(),
                first_timestamp: None,
                last_timestamp: None,
                duration_ms: None,
                error: Some(err.to_string()),
            }),
        }
    }

    summaries.sort_by(|a, b| {
        b.last_timestamp
            .cmp(&a.last_timestamp)
            .then_with(|| a.run_id.cmp(&b.run_id))
    });
    Ok(summaries)
}

pub fn render_harness_event_replay_markdown(events: &[HarnessEvent]) -> String {
    let summary = summarize_harness_events("<memory>", events);
    render_harness_event_replay_markdown_with_summary(&summary, events, &[])
}

pub fn render_harness_event_replay_markdown_with_summary(
    summary: &HarnessEventLogSummary,
    events: &[HarnessEvent],
    diagnostics: &[HarnessEventReadDiagnostic],
) -> String {
    let timeline = build_harness_event_timeline(events);
    let mut output = String::new();
    output.push_str(&format!("# Harness event replay: {}\n\n", summary.run_id));
    output.push_str("## Summary\n\n");
    output.push_str(&format!("- Status: `{}`\n", summary.status));
    output.push_str(&format!("- Events: {}\n", summary.events));
    if let Some(first) = summary.first_timestamp {
        output.push_str(&format!("- Started: `{}`\n", first));
    }
    if let Some(last) = summary.last_timestamp {
        output.push_str(&format!("- Last event: `{}`\n", last));
    }
    if let Some(duration_ms) = summary.duration_ms {
        output.push_str(&format!("- Duration: {} ms\n", duration_ms));
    }
    if let Some(error) = summary.error.as_deref() {
        output.push_str(&format!(
            "- Diagnostics: {}\n",
            escape_markdown_table_cell(error)
        ));
    }

    output.push_str("\n## Diagnostics\n\n");
    if diagnostics.is_empty() {
        output.push_str("- None\n");
    } else {
        for diagnostic in diagnostics {
            output.push_str(&format!(
                "- line {}: {}\n",
                diagnostic.line,
                escape_markdown_table_cell(&diagnostic.message)
            ));
        }
    }

    output.push_str("\n## Failure points\n\n");
    let failures = timeline
        .iter()
        .filter(|item| item.failure)
        .collect::<Vec<_>>();
    if failures.is_empty() {
        output.push_str("- None\n");
    } else {
        for item in failures {
            output.push_str(&format!(
                "- seq {} `{}` `{}`: {}\n",
                item.sequence,
                event_label(&item.kind),
                escape_markdown_table_cell(&item.event_id),
                escape_markdown_table_cell(&item.details),
            ));
        }
    }

    output.push_str("\n## Timeline by phase\n\n");
    for phase in timeline_phase_order(&timeline) {
        let phase_items = timeline
            .iter()
            .filter(|item| item.phase == phase)
            .collect::<Vec<_>>();
        if phase_items.is_empty() {
            continue;
        }
        output.push_str(&format!("### {}\n\n", phase_title(&phase)));
        output.push_str(
            "| Seq | +ms | Level | Kind | Event | Parent | Children | Status | Details |\n",
        );
        output.push_str("| ---: | ---: | --- | --- | --- | --- | ---: | --- | --- |\n");
        for item in phase_items {
            output.push_str(&format!(
                "| {} | {} | `{}` | `{}` | `{}` | {} | {} | `{}` | {} |\n",
                item.sequence,
                item.elapsed_ms
                    .map(|elapsed| elapsed.to_string())
                    .unwrap_or_default(),
                event_label(&item.level),
                event_label(&item.kind),
                escape_markdown_table_cell(&item.event_id),
                item.parent_event_id
                    .as_deref()
                    .map(escape_markdown_table_cell)
                    .unwrap_or_default(),
                item.child_count,
                escape_markdown_table_cell(&item.status),
                escape_markdown_table_cell(&item.details),
            ));
        }
        output.push('\n');
    }
    output
}

pub fn build_harness_event_timeline(events: &[HarnessEvent]) -> Vec<HarnessEventTimelineItem> {
    let first_timestamp = events.iter().map(|event| event.timestamp).min();
    let mut child_counts: HashMap<&str, usize> = HashMap::new();
    for event in events {
        if let Some(parent_event_id) = event.parent_event_id.as_deref() {
            *child_counts.entry(parent_event_id).or_insert(0) += 1;
        }
    }

    events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let failure = event_is_failure(event);
            HarnessEventTimelineItem {
                ordinal: index + 1,
                sequence: event.sequence,
                event_id: event.event_id.clone(),
                parent_event_id: event.parent_event_id.clone(),
                timestamp: event.timestamp,
                elapsed_ms: first_timestamp
                    .and_then(|first| (event.timestamp - first).num_milliseconds().try_into().ok()),
                phase: event_phase(event.kind).to_string(),
                level: event.level,
                kind: event.kind,
                status: event_status(event).to_string(),
                duration_ms: payload_duration_ms(&event.payload),
                child_count: child_counts
                    .get(event.event_id.as_str())
                    .copied()
                    .unwrap_or(0),
                failure,
                details: event_payload_summary(&event.payload),
            }
        })
        .collect()
}

fn event_phase(kind: HarnessEventKind) -> &'static str {
    match kind {
        HarnessEventKind::RunStarted => "run",
        HarnessEventKind::RunCompleted | HarnessEventKind::RunFailed => "completion",
        HarnessEventKind::SkillSelected => "planning",
        HarnessEventKind::MemoryLookupStarted | HarnessEventKind::MemoryLookupFinished => "memory",
        HarnessEventKind::ToolStarted | HarnessEventKind::ToolFinished => "tool_execution",
        HarnessEventKind::FileChanged => "files",
        HarnessEventKind::TestStarted
        | HarnessEventKind::TestPassed
        | HarnessEventKind::TestFailed => "tests",
        HarnessEventKind::GatePassed | HarnessEventKind::GateFailed => "gates",
        HarnessEventKind::HumanApprovalRequired => "approval",
    }
}

fn timeline_phase_order(timeline: &[HarnessEventTimelineItem]) -> Vec<String> {
    let preferred = [
        "run",
        "planning",
        "memory",
        "tool_execution",
        "files",
        "tests",
        "gates",
        "approval",
        "completion",
    ];
    let mut phases = Vec::new();
    for phase in preferred {
        if timeline.iter().any(|item| item.phase == phase) {
            phases.push(phase.to_string());
        }
    }
    for item in timeline {
        if !phases.contains(&item.phase) {
            phases.push(item.phase.clone());
        }
    }
    phases
}

fn phase_title(phase: &str) -> String {
    phase
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn event_status(event: &HarnessEvent) -> &'static str {
    if event_is_failure(event) {
        "failed"
    } else if let Some(status) = event.payload.get("status").and_then(Value::as_str) {
        match status {
            "ok" | "passed" | "completed" => "completed",
            "started" | "running" => "started",
            _ => "info",
        }
    } else {
        match event.kind {
            HarnessEventKind::RunStarted
            | HarnessEventKind::MemoryLookupStarted
            | HarnessEventKind::ToolStarted
            | HarnessEventKind::TestStarted => "started",
            HarnessEventKind::RunCompleted
            | HarnessEventKind::MemoryLookupFinished
            | HarnessEventKind::ToolFinished
            | HarnessEventKind::TestPassed
            | HarnessEventKind::GatePassed => "completed",
            HarnessEventKind::HumanApprovalRequired => "attention_required",
            _ => "info",
        }
    }
}

fn event_is_failure(event: &HarnessEvent) -> bool {
    matches!(
        event.kind,
        HarnessEventKind::RunFailed | HarnessEventKind::TestFailed | HarnessEventKind::GateFailed
    ) || matches!(event.level, HarnessEventLevel::Error)
        || matches!(
            event.payload.get("status").and_then(Value::as_str),
            Some("failed" | "error")
        )
}

fn payload_duration_ms(payload: &Value) -> Option<u128> {
    payload
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(u128::from)
}

fn infer_harness_event_status(events: &[HarnessEvent]) -> &'static str {
    if events.iter().any(event_is_failure) {
        "failed"
    } else if events
        .iter()
        .any(|event| matches!(event.kind, HarnessEventKind::RunCompleted))
    {
        "completed"
    } else if events.is_empty() {
        "empty"
    } else {
        "partial"
    }
}

fn run_id_from_event_log_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("run")
        .to_string()
}

fn event_label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn event_payload_summary(payload: &Value) -> String {
    match payload {
        Value::Object(map) => {
            let mut parts = Vec::new();
            for key in ["status", "tool", "source", "duration_ms", "text_chars"] {
                if let Some(value) = map.get(key) {
                    parts.push(format!("{key}={}", short_json_value(value)));
                }
            }
            if parts.is_empty() {
                "".to_string()
            } else {
                parts.join(", ")
            }
        }
        _ => short_json_value(payload),
    }
}

fn short_json_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn escape_markdown_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn sanitize_run_id(run_id: &str) -> String {
    let sanitized: String = run_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if sanitized.is_empty() {
        "run".to_string()
    } else {
        sanitized
    }
}
pub struct HarnessEventBus {
    sender: broadcast::Sender<HarnessEvent>,
    sequences: Mutex<HashMap<String, u64>>,
}

impl HarnessEventBus {
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<HarnessEventBus> = OnceLock::new();
        INSTANCE.get_or_init(|| Self::with_capacity(DEFAULT_EVENT_BUS_CAPACITY))
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            sequences: Mutex::new(HashMap::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, draft: HarnessEventDraft) -> HarnessEvent {
        let sequence = self.next_sequence(&draft.run_id);
        let event = HarnessEvent {
            schema_version: HARNESS_EVENT_SCHEMA_VERSION,
            event_id: crate::id::new_id("hevt"),
            run_id: draft.run_id,
            session_id: draft.session_id,
            parent_event_id: draft.parent_event_id,
            timestamp: Utc::now(),
            sequence,
            level: draft.level,
            kind: draft.kind,
            payload_class: draft.payload_class,
            payload: redact_harness_event_payload(draft.payload, draft.payload_class),
        };
        let _ = self.sender.send(event.clone());
        event
    }

    pub fn publish_run_started(&self, run_id: impl Into<String>) -> HarnessEvent {
        self.publish(HarnessEventDraft::run_started(run_id))
    }

    pub fn publish_run_completed(&self, run_id: impl Into<String>) -> HarnessEvent {
        self.publish(HarnessEventDraft::run_completed(run_id))
    }

    fn next_sequence(&self, run_id: &str) -> u64 {
        let mut sequences = self
            .sequences
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sequence = sequences.entry(run_id.to_string()).or_insert(0);
        *sequence += 1;
        *sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};

    struct EnvVarRestore {
        key: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvVarRestore {
        fn set_runtime_dir(path: &Path) -> Self {
            let key = "JCODE_RUNTIME_DIR";
            let value = std::env::var_os(key);
            crate::env::set_var(key, path);
            Self { key, value }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            if let Some(value) = self.value.as_ref() {
                crate::env::set_var(self.key, value);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn event_serializes_stable_common_fields() {
        let timestamp = DateTime::parse_from_rfc3339("2026-05-08T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let event = HarnessEvent::new(
            "hevt_test",
            "run_test",
            timestamp,
            7,
            HarnessEventLevel::Info,
            HarnessEventKind::ToolFinished,
            json!({"tool": "cargo test", "status": "passed"}),
        )
        .with_session_id("session_test")
        .with_parent_event_id("hevt_parent");

        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["schema_version"], HARNESS_EVENT_SCHEMA_VERSION);
        assert_eq!(value["event_id"], "hevt_test");
        assert_eq!(value["run_id"], "run_test");
        assert_eq!(value["session_id"], "session_test");
        assert_eq!(value["parent_event_id"], "hevt_parent");
        assert_eq!(value["timestamp"], "2026-05-08T03:00:00Z");
        assert_eq!(value["sequence"], 7);
        assert_eq!(value["level"], "info");
        assert_eq!(value["kind"], "tool_finished");
        assert_eq!(value["payload_class"], "safe_metadata");
        assert_eq!(value["payload"]["tool"], "cargo test");
        assert_eq!(value["payload"]["status"], "passed");
    }

    #[tokio::test]
    async fn event_bus_redacts_sensitive_keys_by_default() {
        let bus = HarnessEventBus::with_capacity(8);

        let event = bus.publish(
            HarnessEventDraft::new("run_redact", HarnessEventKind::ToolStarted).with_payload(
                json!({
                    "tool": "deploy",
                    "api_key": "sk-live-secret",
                    "nested": {
                        "Authorization": "Bearer should-not-leak",
                        "safe": "metadata"
                    },
                    "items": [{"password": "hunter2"}]
                }),
            ),
        );

        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("sk-live-secret"));
        assert!(!serialized.contains("Bearer should-not-leak"));
        assert!(!serialized.contains("hunter2"));
        assert_eq!(event.payload["tool"], "deploy");
        assert_eq!(event.payload["api_key"], HARNESS_EVENT_REDACTED);
        assert_eq!(
            event.payload["nested"]["Authorization"],
            HARNESS_EVENT_REDACTED
        );
        assert_eq!(event.payload["nested"]["safe"], "metadata");
        assert_eq!(
            event.payload["items"][0]["password"],
            HARNESS_EVENT_REDACTED
        );
    }

    #[tokio::test]
    async fn token_usage_metrics_are_not_redacted_but_auth_tokens_are() {
        let bus = HarnessEventBus::with_capacity(8);

        let event = bus.publish(
            HarnessEventDraft::new("run_token_metrics", HarnessEventKind::RunCompleted)
                .with_payload(json!({
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "cache_read_input_tokens": 2,
                    "auth_token": "ghp_should_still_be_redacted",
                })),
        );

        assert_eq!(event.payload["input_tokens"], 10);
        assert_eq!(event.payload["output_tokens"], 4);
        assert_eq!(event.payload["cache_read_input_tokens"], 2);
        assert_eq!(event.payload["auth_token"], HARNESS_EVENT_REDACTED);
    }

    #[tokio::test]
    async fn user_content_payload_is_redacted_wholesale() {
        let bus = HarnessEventBus::with_capacity(8);

        let event = bus.publish(
            HarnessEventDraft::new("run_user_content", HarnessEventKind::HumanApprovalRequired)
                .with_payload_class(HarnessEventPayloadClass::UserContent)
                .with_payload(json!({
                    "prompt": "please do a private thing",
                    "file_content": "secret source text"
                })),
        );

        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("private thing"));
        assert!(!serialized.contains("secret source text"));
        assert_eq!(event.payload_class, HarnessEventPayloadClass::UserContent);
        assert_eq!(event.payload["redacted"], true);
        assert_eq!(event.payload["payload_class"], "user_content");
    }

    #[test]
    fn long_payload_strings_are_truncated() {
        let long = "x".repeat(DEFAULT_MAX_PAYLOAD_STRING_CHARS + 8);

        let redacted = redact_harness_event_payload(
            json!({"summary": long}),
            HarnessEventPayloadClass::SafeMetadata,
        );
        let summary = redacted["summary"].as_str().unwrap();

        assert!(summary.ends_with(HARNESS_EVENT_TRUNCATED));
        assert_eq!(
            summary.chars().count(),
            DEFAULT_MAX_PAYLOAD_STRING_CHARS + HARNESS_EVENT_TRUNCATED.chars().count()
        );
    }

    #[test]
    fn direct_event_constructor_redacts_secret_values() {
        let timestamp = Utc::now();
        let event = HarnessEvent::new(
            "hevt_secret",
            "run_secret",
            timestamp,
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::GatePassed,
            json!({"token": "ghp_should_not_escape", "status": "ok"}),
        );

        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("ghp_should_not_escape"));
        assert_eq!(event.payload["token"], HARNESS_EVENT_REDACTED);
        assert_eq!(event.payload["status"], "ok");
    }

    #[test]
    fn ndjson_writer_emits_one_parseable_line() {
        let timestamp = DateTime::parse_from_rfc3339("2026-05-08T03:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let event = HarnessEvent::new(
            "hevt_ndjson",
            "run_ndjson",
            timestamp,
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::RunStarted,
            json!({"status": "ok"}),
        );
        let mut output = Vec::new();

        write_harness_event_ndjson(&mut output, &event).unwrap();

        let text = String::from_utf8(output).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
        let parsed: HarnessEvent = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(parsed.event_id, "hevt_ndjson");
        assert_eq!(parsed.payload["status"], "ok");
    }

    #[test]
    fn sse_renderer_maps_event_id_type_retry_and_json_data() {
        let event = HarnessEvent::new(
            "hevt_sse",
            "run_sse",
            DateTime::parse_from_rfc3339("2026-05-08T04:22:00Z")
                .unwrap()
                .with_timezone(&Utc),
            3,
            HarnessEventLevel::Info,
            HarnessEventKind::ToolFinished,
            json!({"tool": "cargo test", "status": "ok"}),
        );

        let frame = render_harness_event_sse(&event, Some(DEFAULT_HARNESS_EVENT_SSE_RETRY_MS))
            .expect("sse frame");

        assert!(frame.starts_with("id: hevt_sse\nevent: tool_finished\nretry: 2000\n"));
        assert!(frame.ends_with("\n\n"));
        let data = frame
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("data line");
        let parsed: HarnessEvent = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.event_id, "hevt_sse");
        assert_eq!(parsed.kind, HarnessEventKind::ToolFinished);
        assert_eq!(parsed.payload["tool"], "cargo test");
    }

    #[test]
    fn sse_last_event_id_resume_returns_retained_tail() {
        let bus = HarnessEventBus::with_capacity(8);
        let first = bus.publish(HarnessEventDraft::run_started("run_sse_resume"));
        let second = bus.publish(
            HarnessEventDraft::new("run_sse_resume", HarnessEventKind::ToolFinished)
                .with_payload(json!({"status": "ok"})),
        );
        let third = bus.publish(HarnessEventDraft::run_completed("run_sse_resume"));
        let events = vec![first.clone(), second.clone(), third.clone()];

        assert_eq!(
            harness_events_after_last_event_id(&events, Some(&first.event_id)),
            &[second.clone(), third.clone()]
        );
        assert_eq!(
            harness_events_after_last_event_id(&events, Some(&third.event_id)),
            &[]
        );
        assert_eq!(
            harness_events_after_last_event_id(&events, None),
            events.as_slice()
        );
        assert_eq!(
            harness_events_after_last_event_id(&events, Some("missing_event_id")),
            events.as_slice()
        );
    }

    #[test]
    fn ndjson_append_creates_file_and_preserves_redaction() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-ndjson-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("events").join("run.ndjson");
        let bus = HarnessEventBus::with_capacity(8);

        let first = bus.publish(
            HarnessEventDraft::new("run_file", HarnessEventKind::ToolStarted).with_payload(json!({
                "tool": "bash",
                "token": "ghp_never_write_me"
            })),
        );
        let second = bus.publish(HarnessEventDraft::run_completed("run_file"));

        append_harness_event_ndjson(&path, &first).unwrap();
        append_harness_event_ndjson(&path, &second).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("ghp_never_write_me"));
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first_parsed: HarnessEvent = serde_json::from_str(lines[0]).unwrap();
        let second_parsed: HarnessEvent = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first_parsed.payload["token"], HARNESS_EVENT_REDACTED);
        assert_eq!(first_parsed.sequence, 1);
        assert_eq!(second_parsed.sequence, 2);
    }

    #[test]
    fn ndjson_reader_round_trips_multiple_events() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-read-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("run.ndjson");
        let bus = HarnessEventBus::with_capacity(8);
        let first = bus.publish(HarnessEventDraft::run_started("run_read"));
        let second = bus.publish(HarnessEventDraft::run_completed("run_read"));

        append_harness_event_ndjson(&path, &first).unwrap();
        append_harness_event_ndjson(&path, &second).unwrap();

        let events = read_harness_event_ndjson(&path).unwrap();
        assert_eq!(events, vec![first, second]);
    }

    #[test]
    fn ndjson_reader_reports_corrupt_line_number() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-corrupt-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("run.ndjson");
        let bus = HarnessEventBus::with_capacity(8);

        append_harness_event_ndjson(
            &path,
            &bus.publish(HarnessEventDraft::run_started("run_bad")),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();

        let err = read_harness_event_ndjson(&path).unwrap_err().to_string();
        assert!(err.contains("line 2"), "unexpected error: {err}");
        assert!(
            err.contains("invalid harness event NDJSON"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ndjson_report_preserves_valid_events_after_corrupt_line() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-partial-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("run.ndjson");
        let bus = HarnessEventBus::with_capacity(8);
        let first = bus.publish(HarnessEventDraft::run_started("run_partial"));
        let second = bus.publish(HarnessEventDraft::run_completed("run_partial"));

        append_harness_event_ndjson(&path, &first).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();
        append_harness_event_ndjson(&path, &second).unwrap();

        let report = read_harness_event_ndjson_report(&path).unwrap();
        assert!(report.partial);
        assert_eq!(report.events, vec![first, second]);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].line, 2);
        assert!(report.diagnostics[0].message.contains("line 2"));

        let err = read_harness_event_ndjson(&path).unwrap_err().to_string();
        assert!(err.contains("line 2"), "unexpected error: {err}");
    }

    #[test]
    fn summary_and_replay_surface_truncated_stream_diagnostics() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-truncated-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("run.ndjson");
        let bus = HarnessEventBus::with_capacity(8);
        let started = bus.publish(HarnessEventDraft::run_started("run_truncated"));

        append_harness_event_ndjson(&path, &started).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"schema_version\":1")
            .unwrap();

        let report = read_harness_event_ndjson_report(&path).unwrap();
        let summary = summarize_harness_event_read_report(&report);
        let markdown = render_harness_event_replay_markdown_with_summary(
            &summary,
            &report.events,
            &report.diagnostics,
        );

        assert_eq!(summary.run_id, "run_truncated");
        assert_eq!(summary.events, 1);
        assert_eq!(summary.status, "partial");
        assert!(
            summary
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("line 2")
        );
        assert!(markdown.contains("## Diagnostics"));
        assert!(markdown.contains("line 2"));
        assert!(markdown.contains("invalid harness event NDJSON"));
        assert!(markdown.contains("### Run"));
    }

    #[test]
    fn event_log_path_sanitizes_run_id() {
        let path = harness_event_log_path("run/with spaces/and:*chars");
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();

        assert_eq!(file_name, "run_with_spaces_and__chars.ndjson");
        assert!(path.ends_with("harness-events/run_with_spaces_and__chars.ndjson"));
    }

    #[test]
    fn event_log_summary_detects_completed_status_and_duration() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-summary-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("run_summary.ndjson");
        let started = HarnessEvent::new(
            "hevt_start",
            "run_summary",
            DateTime::parse_from_rfc3339("2026-05-08T03:40:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::RunStarted,
            json!({"status": "started"}),
        );
        let completed = HarnessEvent::new(
            "hevt_done",
            "run_summary",
            DateTime::parse_from_rfc3339("2026-05-08T03:40:02Z")
                .unwrap()
                .with_timezone(&Utc),
            2,
            HarnessEventLevel::Info,
            HarnessEventKind::RunCompleted,
            json!({"status": "ok", "duration_ms": 2000}),
        );

        append_harness_event_ndjson(&path, &started).unwrap();
        append_harness_event_ndjson(&path, &completed).unwrap();

        let summary = summarize_harness_event_log(&path).unwrap();
        assert_eq!(summary.run_id, "run_summary");
        assert_eq!(summary.events, 2);
        assert_eq!(summary.status, "completed");
        assert_eq!(summary.duration_ms, Some(2000));
    }

    #[test]
    fn list_event_logs_includes_corrupt_diagnostics() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-list-")
            .tempdir()
            .unwrap();
        let _env = EnvVarRestore::set_runtime_dir(temp.path());
        std::fs::create_dir_all(default_harness_event_log_dir()).unwrap();
        let bus = HarnessEventBus::with_capacity(8);
        append_harness_event_ndjson(
            harness_event_log_path("run_good"),
            &bus.publish(HarnessEventDraft::run_completed("run_good")),
        )
        .unwrap();
        std::fs::write(
            default_harness_event_log_dir().join("run_bad.ndjson"),
            "bad\n",
        )
        .unwrap();

        let summaries = list_harness_event_logs().unwrap();
        assert!(summaries.iter().any(|summary| summary.run_id == "run_good"));
        let corrupt = summaries
            .iter()
            .find(|summary| summary.run_id == "run_bad")
            .expect("corrupt summary");
        assert_eq!(corrupt.status, "corrupt");
        assert!(
            corrupt
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("line 1")
        );
    }

    #[test]
    fn replay_markdown_contains_summary_and_timeline() {
        let event = HarnessEvent::new(
            "hevt_tool",
            "run_replay",
            DateTime::parse_from_rfc3339("2026-05-08T03:41:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::ToolFinished,
            json!({"tool": "cargo test", "status": "ok"}),
        );

        let markdown = render_harness_event_replay_markdown(&[event]);

        assert!(markdown.contains("# Harness event replay: run_replay"));
        assert!(markdown.contains("## Summary"));
        assert!(markdown.contains("## Failure points"));
        assert!(markdown.contains("## Timeline by phase"));
        assert!(markdown.contains("### Tool Execution"));
        assert!(markdown.contains(
            "| Seq | +ms | Level | Kind | Event | Parent | Children | Status | Details |"
        ));
        assert!(markdown.contains("tool=cargo test"));
    }

    #[test]
    fn replay_markdown_matches_stable_snapshot() {
        let event = HarnessEvent::new(
            "hevt_tool",
            "run_replay_snapshot",
            DateTime::parse_from_rfc3339("2026-05-08T03:41:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::ToolFinished,
            json!({"tool": "cargo test", "status": "ok"}),
        );

        let markdown = render_harness_event_replay_markdown(&[event]);

        assert_eq!(
            markdown,
            "# Harness event replay: run_replay_snapshot\n\
\n\
## Summary\n\
\n\
- Status: `partial`\n\
- Events: 1\n\
- Started: `2026-05-08 03:41:00 UTC`\n\
- Last event: `2026-05-08 03:41:00 UTC`\n\
- Duration: 0 ms\n\
\n\
## Diagnostics\n\
\n\
- None\n\
\n\
## Failure points\n\
\n\
- None\n\
\n\
## Timeline by phase\n\
\n\
### Tool Execution\n\
\n\
| Seq | +ms | Level | Kind | Event | Parent | Children | Status | Details |\n\
| ---: | ---: | --- | --- | --- | --- | ---: | --- | --- |\n\
| 1 | 0 | `info` | `tool_finished` | `hevt_tool` |  | 0 | `completed` | status=ok, tool=cargo test |\n\
\n"
        );
    }

    #[test]
    fn benchmark_report_covers_core_event_paths() {
        let report = run_harness_event_benchmark(HarnessEventBenchmarkOptions { events: 16 })
            .expect("benchmark should run");

        assert_eq!(report.events, 16);
        assert!(report.ndjson_bytes > report.events);
        assert_eq!(report.read_diagnostics, 0);
        assert_metric_is_finite(&report.publish_no_subscribers);
        assert_metric_is_finite(&report.ndjson_write_memory);
        assert_metric_is_finite(&report.ndjson_write_file);
        assert_metric_is_finite(&report.ndjson_read_report_file);
        assert_metric_is_finite(&report.timeline_build);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("Synthetic single-process baseline"))
        );
    }

    fn assert_metric_is_finite(metric: &HarnessEventBenchmarkMetric) {
        assert!(metric.total_nanos > 0);
        assert!(metric.micros_per_event.is_finite());
        assert!(metric.micros_per_event >= 0.0);
        assert!(metric.events_per_second.is_finite() || metric.events_per_second.is_infinite());
        assert!(metric.events_per_second > 0.0);
    }

    #[test]
    fn timeline_items_include_phase_elapsed_parent_child_and_failure() {
        let start = HarnessEvent::new(
            "hevt_start",
            "run_timeline",
            DateTime::parse_from_rfc3339("2026-05-08T03:42:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::RunStarted,
            json!({"status": "started"}),
        );
        let tool = HarnessEvent::new(
            "hevt_tool",
            "run_timeline",
            DateTime::parse_from_rfc3339("2026-05-08T03:42:01Z")
                .unwrap()
                .with_timezone(&Utc),
            2,
            HarnessEventLevel::Error,
            HarnessEventKind::ToolFinished,
            json!({"tool": "bash", "status": "failed", "duration_ms": 1000}),
        )
        .with_parent_event_id("hevt_start");

        let timeline = build_harness_event_timeline(&[start, tool]);

        assert_eq!(timeline[0].phase, "run");
        assert_eq!(timeline[0].elapsed_ms, Some(0));
        assert_eq!(timeline[0].child_count, 1);
        assert_eq!(timeline[1].phase, "tool_execution");
        assert_eq!(timeline[1].elapsed_ms, Some(1000));
        assert_eq!(timeline[1].parent_event_id.as_deref(), Some("hevt_start"));
        assert_eq!(timeline[1].duration_ms, Some(1000));
        assert_eq!(timeline[1].status, "failed");
        assert!(timeline[1].failure);
    }

    #[tokio::test]
    async fn event_bus_assigns_monotonic_sequences_per_run() {
        let bus = HarnessEventBus::with_capacity(8);
        let mut rx = bus.subscribe();

        let first = bus.publish(HarnessEventDraft::run_started("run_a"));
        let second = bus.publish(HarnessEventDraft::run_completed("run_a"));
        let other_run = bus.publish(HarnessEventDraft::run_started("run_b"));

        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(other_run.sequence, 1);

        assert_eq!(rx.recv().await.unwrap().sequence, 1);
        assert_eq!(rx.recv().await.unwrap().sequence, 2);
        assert_eq!(rx.recv().await.unwrap().run_id, "run_b");
    }

    #[tokio::test]
    async fn event_bus_fans_out_to_multiple_subscribers() {
        let bus = HarnessEventBus::with_capacity(8);
        let mut first_rx = bus.subscribe();
        let mut second_rx = bus.subscribe();

        let event = bus.publish(
            HarnessEventDraft::new("run_fanout", HarnessEventKind::HumanApprovalRequired)
                .with_payload(json!({"reason": "approval required"})),
        );

        let first_seen = timeout(Duration::from_secs(1), first_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let second_seen = timeout(Duration::from_secs(1), second_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(first_seen.event_id, event.event_id);
        assert_eq!(second_seen.event_id, event.event_id);
        assert_eq!(first_seen.kind, HarnessEventKind::HumanApprovalRequired);
        assert_eq!(second_seen.payload["reason"], "approval required");
    }

    #[tokio::test]
    async fn slow_subscribers_lag_instead_of_blocking_publish() {
        let bus = HarnessEventBus::with_capacity(4);
        let mut slow_rx = bus.subscribe();

        for index in 0..32 {
            let event = bus.publish(
                HarnessEventDraft::new("run_slow_subscriber", HarnessEventKind::ToolFinished)
                    .with_payload(json!({"status": "ok", "index": index})),
            );
            assert_eq!(event.sequence, index + 1);
        }

        let lagged = timeout(Duration::from_secs(1), slow_rx.recv())
            .await
            .expect("lagged receiver should respond")
            .expect_err("slow receiver should report lag instead of blocking publisher");
        match lagged {
            tokio::sync::broadcast::error::RecvError::Lagged(skipped) => assert!(skipped > 0),
            other => panic!("unexpected receive error: {other:?}"),
        }

        let retained = timeout(Duration::from_secs(1), slow_rx.recv())
            .await
            .expect("lagged receiver should recover")
            .expect("retained tail event should be readable");
        assert!(retained.sequence > 28);
    }

    #[test]
    fn publishing_without_subscribers_still_returns_event() {
        let bus = HarnessEventBus::with_capacity(8);

        let event = bus.publish(HarnessEventDraft::run_started("run_no_subscribers"));

        assert_eq!(event.sequence, 1);
        assert_eq!(event.kind, HarnessEventKind::RunStarted);
        assert!(event.event_id.starts_with("hevt_"));
    }
}
