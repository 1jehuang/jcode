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
pub const HARNESS_EVENT_BROKER_PROTOCOL_VERSION: u16 = 1;
pub const HARNESS_EVENT_BROKER_DELIVERY_SEMANTICS: &str = "at_least_once";
pub const HARNESS_EVENT_MIN_LEVEL_ENV: &str = "JCODE_HARNESS_EVENTS_MIN_LEVEL";
pub const HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV: &str = "JCODE_HARNESS_EVENTS_TOOL_SAMPLE_EVERY";
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

impl HarnessEventLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Trace => 0,
            Self::Debug => 1,
            Self::Info => 2,
            Self::Warn => 3,
            Self::Error => 4,
        }
    }

    pub fn is_at_least(self, minimum: Self) -> bool {
        self.rank() >= minimum.rank()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
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
    HumanApprovalResolved,
    ControlCommandReceived,
    ControlCommandRejected,
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventRetentionPolicy {
    pub max_logs: Option<usize>,
    pub max_total_bytes: Option<u64>,
    pub dry_run: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventRetentionEntry {
    pub run_id: String,
    pub path: String,
    pub bytes: u64,
    pub modified_timestamp: Option<DateTime<Utc>>,
    pub reason: String,
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarnessEventRetentionReport {
    pub log_dir: String,
    pub dry_run: bool,
    pub before_logs: usize,
    pub before_bytes: u64,
    pub kept_logs: usize,
    pub kept_bytes: u64,
    pub pruned_logs: usize,
    pub pruned_bytes: u64,
    pub candidates: Vec<HarnessEventRetentionEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HarnessEventSamplingPolicy {
    pub min_level: HarnessEventLevel,
    pub tool_event_sample_every: Option<u64>,
}

impl Default for HarnessEventSamplingPolicy {
    fn default() -> Self {
        Self {
            min_level: HarnessEventLevel::Trace,
            tool_event_sample_every: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HarnessEventSamplingDecision {
    pub record: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HarnessEventSinkAck {
    pub sink: String,
    pub durable: bool,
    pub event_id: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessEventBrokerRoute {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub nats_subject: String,
    pub redis_stream_key: String,
    pub durable_consumer: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventAckOutcome {
    Pending,
    Acked,
    Nacked,
    Redelivered,
    Dropped,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HarnessEventBrokerEnvelope {
    pub protocol_version: u16,
    pub delivery_semantics: String,
    pub dedupe_key: String,
    pub event: HarnessEvent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HarnessEventDelivery {
    pub route: HarnessEventBrokerRoute,
    pub envelope: HarnessEventBrokerEnvelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub redelivered: bool,
    pub delivery_attempt: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HarnessEventDeliveryAck {
    pub outcome: HarnessEventAckOutcome,
    pub dedupe_key: String,
    pub event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub trait HarnessEventSink {
    fn sink_name(&self) -> &str;
    fn publish_event(&mut self, event: &HarnessEvent) -> anyhow::Result<HarnessEventSinkAck>;
    fn flush(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub trait HarnessEventSource {
    fn source_name(&self) -> &str;
    fn read_events_after(
        &self,
        run_id: &str,
        last_event_id: Option<&str>,
    ) -> anyhow::Result<Vec<HarnessEvent>>;
}

#[derive(Clone, Debug)]
pub struct HarnessEventNdjsonSink {
    path: PathBuf,
}

impl HarnessEventNdjsonSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn for_run(run_id: &str) -> Self {
        Self::new(harness_event_log_path(run_id))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug)]
pub struct HarnessEventNdjsonSource {
    log_dir: PathBuf,
}

impl HarnessEventNdjsonSource {
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self {
            log_dir: log_dir.into(),
        }
    }

    pub fn default_dir() -> Self {
        Self::new(default_harness_event_log_dir())
    }

    pub fn event_log_path(&self, run_id: &str) -> PathBuf {
        self.log_dir
            .join(format!("{}.ndjson", sanitize_run_id(run_id)))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessApprovalDecision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum HarnessControlCommand {
    SubscribeEvents {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_event_id: Option<String>,
    },
    ResolveHumanApproval {
        run_id: String,
        approval_id: String,
        decision: HarnessApprovalDecision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_command_id: Option<String>,
    },
    PauseRun {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_command_id: Option<String>,
    },
    ResumeRun {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_command_id: Option<String>,
    },
    CancelRun {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_command_id: Option<String>,
    },
    UiCommand {
        run_id: String,
        name: String,
        #[serde(default)]
        args: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_command_id: Option<String>,
    },
}

#[derive(Clone, Debug)]
struct HarnessEventRetentionLogEntry {
    run_id: String,
    path: PathBuf,
    bytes: u64,
    modified_timestamp: Option<DateTime<Utc>>,
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

    pub fn level(&self) -> HarnessEventLevel {
        self.level
    }

    pub fn kind(&self) -> HarnessEventKind {
        self.kind
    }
}

impl HarnessEventSamplingPolicy {
    pub fn from_env() -> anyhow::Result<Self> {
        let mut policy = Self::default();

        if let Ok(value) = std::env::var(HARNESS_EVENT_MIN_LEVEL_ENV) {
            policy.min_level = HarnessEventLevel::parse(&value).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid {HARNESS_EVENT_MIN_LEVEL_ENV}: expected trace, debug, info, warn, or error"
                )
            })?;
        }

        if let Ok(value) = std::env::var(HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV) {
            let sample_every = value.trim().parse::<u64>().map_err(|_| {
                anyhow::anyhow!("invalid {HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV}: expected integer")
            })?;
            if sample_every == 0 {
                anyhow::bail!(
                    "invalid {HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV}: expected value greater than zero"
                );
            }
            policy.tool_event_sample_every = (sample_every > 1).then_some(sample_every);
        }

        Ok(policy)
    }

    pub fn should_record(
        &self,
        draft: &HarnessEventDraft,
        same_kind_ordinal: u64,
    ) -> HarnessEventSamplingDecision {
        if !draft.level().is_at_least(self.min_level) {
            return HarnessEventSamplingDecision {
                record: false,
                reason: Some("below_min_level".to_string()),
            };
        }

        if draft.level().is_at_least(HarnessEventLevel::Warn) {
            return HarnessEventSamplingDecision {
                record: true,
                reason: None,
            };
        }

        if is_tool_sampling_kind(draft.kind()) {
            if let Some(sample_every) = self.tool_event_sample_every {
                if same_kind_ordinal.saturating_sub(1) % sample_every != 0 {
                    return HarnessEventSamplingDecision {
                        record: false,
                        reason: Some("tool_sample_every".to_string()),
                    };
                }
            }
        }

        HarnessEventSamplingDecision {
            record: true,
            reason: None,
        }
    }
}

impl HarnessControlCommand {
    pub fn parse_json(input: &str) -> anyhow::Result<Self> {
        let command = serde_json::from_str::<Self>(input)
            .map_err(|err| anyhow::anyhow!("invalid harness control command JSON: {err}"))?;
        command.validate()?;
        Ok(command)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        require_control_field("run_id", self.run_id())?;
        match self {
            Self::SubscribeEvents { last_event_id, .. } => {
                if let Some(last_event_id) = last_event_id {
                    require_control_field("last_event_id", last_event_id)?;
                }
            }
            Self::ResolveHumanApproval {
                approval_id,
                actor,
                reason,
                client_command_id,
                ..
            } => {
                require_control_field("approval_id", approval_id)?;
                validate_optional_control_field("actor", actor)?;
                validate_optional_control_field("reason", reason)?;
                validate_optional_control_field("client_command_id", client_command_id)?;
            }
            Self::PauseRun {
                actor,
                reason,
                client_command_id,
                ..
            }
            | Self::ResumeRun {
                actor,
                reason,
                client_command_id,
                ..
            }
            | Self::CancelRun {
                actor,
                reason,
                client_command_id,
                ..
            } => {
                validate_optional_control_field("actor", actor)?;
                validate_optional_control_field("reason", reason)?;
                validate_optional_control_field("client_command_id", client_command_id)?;
            }
            Self::UiCommand {
                name,
                client_command_id,
                ..
            } => {
                require_control_field("name", name)?;
                validate_optional_control_field("client_command_id", client_command_id)?;
            }
        }
        Ok(())
    }

    pub fn run_id(&self) -> &str {
        match self {
            Self::SubscribeEvents { run_id, .. }
            | Self::ResolveHumanApproval { run_id, .. }
            | Self::PauseRun { run_id, .. }
            | Self::ResumeRun { run_id, .. }
            | Self::CancelRun { run_id, .. }
            | Self::UiCommand { run_id, .. } => run_id,
        }
    }

    pub fn command_name(&self) -> &'static str {
        match self {
            Self::SubscribeEvents { .. } => "subscribe_events",
            Self::ResolveHumanApproval { .. } => "resolve_human_approval",
            Self::PauseRun { .. } => "pause_run",
            Self::ResumeRun { .. } => "resume_run",
            Self::CancelRun { .. } => "cancel_run",
            Self::UiCommand { .. } => "ui_command",
        }
    }

    pub fn client_command_id(&self) -> Option<&str> {
        match self {
            Self::SubscribeEvents { .. } => None,
            Self::ResolveHumanApproval {
                client_command_id, ..
            }
            | Self::PauseRun {
                client_command_id, ..
            }
            | Self::ResumeRun {
                client_command_id, ..
            }
            | Self::CancelRun {
                client_command_id, ..
            }
            | Self::UiCommand {
                client_command_id, ..
            } => client_command_id.as_deref(),
        }
    }

    pub fn requires_write_authorization(&self) -> bool {
        !matches!(self, Self::SubscribeEvents { .. })
    }
}

pub fn harness_control_command_event_draft(
    command: &HarnessControlCommand,
    write_authorized: bool,
) -> HarnessEventDraft {
    let authorized = write_authorized || !command.requires_write_authorization();
    let mut payload = json!({
        "command": command.command_name(),
        "authorized": authorized,
    });

    if let Some(client_command_id) = command.client_command_id() {
        payload["client_command_id"] = Value::String(client_command_id.to_string());
    }

    let (kind, level, status) = if !authorized {
        (
            HarnessEventKind::ControlCommandRejected,
            HarnessEventLevel::Warn,
            "rejected",
        )
    } else if matches!(command, HarnessControlCommand::ResolveHumanApproval { .. }) {
        (
            HarnessEventKind::HumanApprovalResolved,
            HarnessEventLevel::Info,
            "resolved",
        )
    } else {
        (
            HarnessEventKind::ControlCommandReceived,
            HarnessEventLevel::Info,
            "accepted",
        )
    };
    payload["status"] = Value::String(status.to_string());

    match command {
        HarnessControlCommand::SubscribeEvents { last_event_id, .. } => {
            if let Some(last_event_id) = last_event_id {
                payload["last_event_id"] = Value::String(last_event_id.clone());
            }
        }
        HarnessControlCommand::ResolveHumanApproval {
            approval_id,
            decision,
            actor,
            reason,
            ..
        } => {
            payload["approval_id"] = Value::String(approval_id.clone());
            payload["decision"] = json!(decision);
            payload["approved"] =
                Value::Bool(matches!(decision, HarnessApprovalDecision::Approved));
            if let Some(actor) = actor {
                payload["actor"] = Value::String(actor.clone());
            }
            payload["reason_present"] = Value::Bool(reason.is_some());
        }
        HarnessControlCommand::PauseRun { actor, reason, .. }
        | HarnessControlCommand::ResumeRun { actor, reason, .. }
        | HarnessControlCommand::CancelRun { actor, reason, .. } => {
            if let Some(actor) = actor {
                payload["actor"] = Value::String(actor.clone());
            }
            payload["reason_present"] = Value::Bool(reason.is_some());
        }
        HarnessControlCommand::UiCommand { name, args, .. } => {
            payload["name"] = Value::String(name.clone());
            payload["args"] = args.clone();
        }
    }

    HarnessEventDraft::new(command.run_id(), kind)
        .with_level(level)
        .with_payload(payload)
}

fn require_control_field(name: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("invalid harness control command: {name} must not be empty");
    }
    Ok(())
}

fn validate_optional_control_field(name: &str, value: &Option<String>) -> anyhow::Result<()> {
    if let Some(value) = value {
        require_control_field(name, value)?;
    }
    Ok(())
}

fn is_tool_sampling_kind(kind: HarnessEventKind) -> bool {
    matches!(
        kind,
        HarnessEventKind::ToolStarted | HarnessEventKind::ToolFinished
    )
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

impl HarnessEventSink for HarnessEventNdjsonSink {
    fn sink_name(&self) -> &str {
        "ndjson"
    }

    fn publish_event(&mut self, event: &HarnessEvent) -> anyhow::Result<HarnessEventSinkAck> {
        append_harness_event_ndjson(&self.path, event)?;
        Ok(HarnessEventSinkAck {
            sink: self.sink_name().to_string(),
            durable: true,
            event_id: event.event_id.clone(),
            run_id: event.run_id.clone(),
            message_id: Some(format!("{}:{}", event.run_id, event.sequence)),
        })
    }
}

impl HarnessEventSource for HarnessEventNdjsonSource {
    fn source_name(&self) -> &str {
        "ndjson"
    }

    fn read_events_after(
        &self,
        run_id: &str,
        last_event_id: Option<&str>,
    ) -> anyhow::Result<Vec<HarnessEvent>> {
        let path = self.event_log_path(run_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let events = read_harness_event_ndjson(path)?;
        Ok(harness_events_after_last_event_id(&events, last_event_id).to_vec())
    }
}

pub fn harness_event_broker_route(event: &HarnessEvent) -> HarnessEventBrokerRoute {
    let run_id = encode_harness_event_broker_token(&event.run_id);
    let session_id = event
        .session_id
        .as_deref()
        .map(encode_harness_event_broker_token)
        .filter(|value| !value.is_empty());
    let task_id = event
        .payload
        .get("task_id")
        .and_then(Value::as_str)
        .map(encode_harness_event_broker_token)
        .filter(|value| !value.is_empty());

    let mut subject_parts = vec!["jcode", "harness_events", "v1", "run"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    subject_parts.push(run_id.clone());
    if let Some(session_id) = &session_id {
        subject_parts.push("session".to_string());
        subject_parts.push(session_id.clone());
    }
    if let Some(task_id) = &task_id {
        subject_parts.push("task".to_string());
        subject_parts.push(task_id.clone());
    }

    let mut redis_stream_key = format!("jcode:harness-events:v1:run:{run_id}:events");
    if let Some(session_id) = &session_id {
        redis_stream_key.push_str(":session:");
        redis_stream_key.push_str(session_id);
    }
    if let Some(task_id) = &task_id {
        redis_stream_key.push_str(":task:");
        redis_stream_key.push_str(task_id);
    }
    let durable_consumer = format!("jcode-harness-{}", run_id);

    HarnessEventBrokerRoute {
        run_id,
        session_id,
        task_id,
        nats_subject: subject_parts.join("."),
        redis_stream_key,
        durable_consumer,
    }
}

pub fn harness_event_dedupe_key(event: &HarnessEvent) -> String {
    format!("v{}:{}", event.schema_version, event.event_id)
}

pub fn harness_event_broker_envelope(event: &HarnessEvent) -> HarnessEventBrokerEnvelope {
    HarnessEventBrokerEnvelope {
        protocol_version: HARNESS_EVENT_BROKER_PROTOCOL_VERSION,
        delivery_semantics: HARNESS_EVENT_BROKER_DELIVERY_SEMANTICS.to_string(),
        dedupe_key: harness_event_dedupe_key(event),
        event: event.clone(),
    }
}

pub fn serialize_harness_event_broker_payload(event: &HarnessEvent) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&harness_event_broker_envelope(event))?)
}

pub fn deserialize_harness_event_broker_payload(
    payload: &[u8],
) -> anyhow::Result<HarnessEventBrokerEnvelope> {
    let envelope = serde_json::from_slice::<HarnessEventBrokerEnvelope>(payload)?;
    if envelope.protocol_version != HARNESS_EVENT_BROKER_PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported harness event broker protocol version {}",
            envelope.protocol_version
        );
    }
    if envelope.delivery_semantics != HARNESS_EVENT_BROKER_DELIVERY_SEMANTICS {
        anyhow::bail!(
            "unsupported harness event broker delivery semantics {}",
            envelope.delivery_semantics
        );
    }
    if envelope.dedupe_key != harness_event_dedupe_key(&envelope.event) {
        anyhow::bail!("invalid harness event broker payload dedupe key");
    }
    Ok(envelope)
}

pub fn harness_event_delivery(
    event: &HarnessEvent,
    message_id: Option<String>,
    redelivered: bool,
    delivery_attempt: u32,
) -> HarnessEventDelivery {
    HarnessEventDelivery {
        route: harness_event_broker_route(event),
        envelope: harness_event_broker_envelope(event),
        message_id,
        redelivered,
        delivery_attempt: delivery_attempt.max(1),
    }
}

pub fn harness_event_delivery_ack(
    delivery: &HarnessEventDelivery,
    outcome: HarnessEventAckOutcome,
    detail: Option<String>,
) -> HarnessEventDeliveryAck {
    HarnessEventDeliveryAck {
        outcome,
        dedupe_key: delivery.envelope.dedupe_key.clone(),
        event_id: delivery.envelope.event.event_id.clone(),
        message_id: delivery.message_id.clone(),
        detail,
    }
}

pub fn encode_harness_event_broker_token(value: &str) -> String {
    if value.is_empty() {
        return "b00".to_string();
    }
    let mut encoded = String::with_capacity(1 + value.len().saturating_mul(2));
    encoded.push('b');
    encoded.push_str(&hex::encode(value.as_bytes()));
    encoded
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

pub fn apply_harness_event_log_retention(
    policy: HarnessEventRetentionPolicy,
) -> anyhow::Result<HarnessEventRetentionReport> {
    apply_harness_event_log_retention_in_dir(default_harness_event_log_dir(), policy)
}

pub fn apply_harness_event_log_retention_in_dir(
    log_dir: impl AsRef<Path>,
    policy: HarnessEventRetentionPolicy,
) -> anyhow::Result<HarnessEventRetentionReport> {
    let log_dir = log_dir.as_ref();
    if policy.max_logs.is_none() && policy.max_total_bytes.is_none() {
        anyhow::bail!("retention policy must set max_logs or max_total_bytes");
    }

    let mut entries = collect_harness_event_retention_entries(log_dir)?;
    entries.sort_by(|a, b| {
        b.modified_timestamp
            .cmp(&a.modified_timestamp)
            .then_with(|| a.path.cmp(&b.path))
    });

    let before_logs = entries.len();
    let before_bytes = entries.iter().map(|entry| entry.bytes).sum();
    let mut kept_logs = 0usize;
    let mut kept_bytes = 0u64;
    let mut candidates = Vec::new();

    for entry in entries {
        let reason = if policy
            .max_logs
            .is_some_and(|max_logs| kept_logs >= max_logs)
        {
            Some("max_logs")
        } else if policy
            .max_total_bytes
            .is_some_and(|max_bytes| kept_bytes.saturating_add(entry.bytes) > max_bytes)
        {
            Some("max_total_bytes")
        } else {
            None
        };

        if let Some(reason) = reason {
            if !policy.dry_run {
                std::fs::remove_file(&entry.path)?;
            }
            candidates.push(HarnessEventRetentionEntry {
                run_id: entry.run_id,
                path: entry.path.display().to_string(),
                bytes: entry.bytes,
                modified_timestamp: entry.modified_timestamp,
                reason: reason.to_string(),
                deleted: !policy.dry_run,
            });
        } else {
            kept_logs += 1;
            kept_bytes = kept_bytes.saturating_add(entry.bytes);
        }
    }

    let pruned_logs = candidates.len();
    let pruned_bytes = candidates.iter().map(|entry| entry.bytes).sum();
    Ok(HarnessEventRetentionReport {
        log_dir: log_dir.display().to_string(),
        dry_run: policy.dry_run,
        before_logs,
        before_bytes,
        kept_logs,
        kept_bytes,
        pruned_logs,
        pruned_bytes,
        candidates,
    })
}

fn collect_harness_event_retention_entries(
    log_dir: &Path,
) -> anyhow::Result<Vec<HarnessEventRetentionLogEntry>> {
    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("ndjson") {
            continue;
        }
        let metadata = entry.metadata()?;
        entries.push(HarnessEventRetentionLogEntry {
            run_id: run_id_from_event_log_path(&path),
            path,
            bytes: metadata.len(),
            modified_timestamp: metadata.modified().ok().map(DateTime::<Utc>::from),
        });
    }
    Ok(entries)
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
        HarnessEventKind::HumanApprovalRequired | HarnessEventKind::HumanApprovalResolved => {
            "approval"
        }
        HarnessEventKind::ControlCommandReceived | HarnessEventKind::ControlCommandRejected => {
            "control"
        }
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
        "control",
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
            HarnessEventKind::HumanApprovalResolved => "resolved",
            HarnessEventKind::ControlCommandReceived => "accepted",
            HarnessEventKind::ControlCommandRejected => "rejected",
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
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            crate::env::remove_var(key);
            Self {
                key,
                value: previous,
            }
        }

        fn set_runtime_dir(path: &Path) -> Self {
            Self::set("JCODE_RUNTIME_DIR", path)
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

    fn write_retention_test_log(dir: &Path, run_id: &str, event_id: &str) -> PathBuf {
        let path = dir.join(format!("{run_id}.ndjson"));
        let event = HarnessEvent::new(
            event_id,
            run_id,
            DateTime::parse_from_rfc3339("2026-05-08T04:40:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::RunCompleted,
            json!({"status": "ok"}),
        );
        append_harness_event_ndjson(&path, &event).unwrap();
        path
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
    fn sampling_policy_drops_below_min_level_and_keeps_warnings() {
        let policy = HarnessEventSamplingPolicy {
            min_level: HarnessEventLevel::Warn,
            tool_event_sample_every: None,
        };
        let info = HarnessEventDraft::new("run_sampling", HarnessEventKind::ToolStarted);
        let warn = HarnessEventDraft::new("run_sampling", HarnessEventKind::GateFailed)
            .with_level(HarnessEventLevel::Warn);

        let info_decision = policy.should_record(&info, 1);
        let warn_decision = policy.should_record(&warn, 1);

        assert!(!info_decision.record);
        assert_eq!(info_decision.reason.as_deref(), Some("below_min_level"));
        assert!(warn_decision.record);
    }

    #[test]
    fn sampling_policy_samples_tool_events_deterministically() {
        let policy = HarnessEventSamplingPolicy {
            min_level: HarnessEventLevel::Trace,
            tool_event_sample_every: Some(3),
        };
        let tool = HarnessEventDraft::new("run_sampling", HarnessEventKind::ToolStarted);
        let run = HarnessEventDraft::run_started("run_sampling");

        assert!(policy.should_record(&tool, 1).record);
        assert!(!policy.should_record(&tool, 2).record);
        assert!(!policy.should_record(&tool, 3).record);
        assert!(policy.should_record(&tool, 4).record);
        assert!(policy.should_record(&run, 2).record);
    }

    #[test]
    fn sampling_policy_from_env_parses_knobs_and_rejects_zero() {
        let _lock = crate::storage::lock_test_env();
        let min_level = EnvVarRestore::set(HARNESS_EVENT_MIN_LEVEL_ENV, "warn");
        let sample_every = EnvVarRestore::set(HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV, "5");

        let policy = HarnessEventSamplingPolicy::from_env().unwrap();

        assert_eq!(policy.min_level, HarnessEventLevel::Warn);
        assert_eq!(policy.tool_event_sample_every, Some(5));

        drop(sample_every);
        let _sample_zero = EnvVarRestore::set(HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV, "0");
        let err = HarnessEventSamplingPolicy::from_env()
            .unwrap_err()
            .to_string();
        assert!(err.contains(HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV));
        drop(min_level);
    }

    #[test]
    fn sampling_policy_from_env_defaults_when_unset() {
        let _lock = crate::storage::lock_test_env();
        let _min_level = EnvVarRestore::remove(HARNESS_EVENT_MIN_LEVEL_ENV);
        let _sample_every = EnvVarRestore::remove(HARNESS_EVENT_TOOL_SAMPLE_EVERY_ENV);

        let policy = HarnessEventSamplingPolicy::from_env().unwrap();

        assert_eq!(policy, HarnessEventSamplingPolicy::default());
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
    fn retention_dry_run_reports_prunable_logs_without_deleting() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-retention-dry-run-")
            .tempdir()
            .unwrap();
        let dir = temp.path().join("harness-events");
        let old = write_retention_test_log(&dir, "run_old", "hevt_old");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let new = write_retention_test_log(&dir, "run_new", "hevt_new");

        let report = apply_harness_event_log_retention_in_dir(
            &dir,
            HarnessEventRetentionPolicy {
                max_logs: Some(1),
                max_total_bytes: None,
                dry_run: true,
            },
        )
        .unwrap();

        assert_eq!(report.before_logs, 2);
        assert_eq!(report.kept_logs, 1);
        assert_eq!(report.pruned_logs, 1);
        assert_eq!(report.candidates[0].run_id, "run_old");
        assert_eq!(report.candidates[0].reason, "max_logs");
        assert!(!report.candidates[0].deleted);
        assert!(old.exists());
        assert!(new.exists());
    }

    #[test]
    fn retention_apply_deletes_old_logs() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-retention-apply-")
            .tempdir()
            .unwrap();
        let dir = temp.path().join("harness-events");
        let old = write_retention_test_log(&dir, "run_old", "hevt_old");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let new = write_retention_test_log(&dir, "run_new", "hevt_new");

        let report = apply_harness_event_log_retention_in_dir(
            &dir,
            HarnessEventRetentionPolicy {
                max_logs: Some(1),
                max_total_bytes: None,
                dry_run: false,
            },
        )
        .unwrap();

        assert_eq!(report.pruned_logs, 1);
        assert_eq!(report.candidates[0].run_id, "run_old");
        assert!(report.candidates[0].deleted);
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn retention_max_total_bytes_limits_kept_logs() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-retention-bytes-")
            .tempdir()
            .unwrap();
        let dir = temp.path().join("harness-events");
        let old = write_retention_test_log(&dir, "run_old", "hevt_old");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let new = write_retention_test_log(&dir, "run_new", "hevt_new");
        let new_bytes = std::fs::metadata(&new).unwrap().len();

        let report = apply_harness_event_log_retention_in_dir(
            &dir,
            HarnessEventRetentionPolicy {
                max_logs: None,
                max_total_bytes: Some(new_bytes),
                dry_run: true,
            },
        )
        .unwrap();

        assert_eq!(report.kept_logs, 1);
        assert_eq!(report.kept_bytes, new_bytes);
        assert_eq!(report.pruned_logs, 1);
        assert_eq!(report.candidates[0].run_id, "run_old");
        assert_eq!(report.candidates[0].reason, "max_total_bytes");
        assert!(old.exists());
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
    fn control_command_parser_accepts_subscribe_and_rejects_empty_fields() {
        let subscribe = HarnessControlCommand::parse_json(
            r#"{"command":"subscribe_events","run_id":"run_ws","last_event_id":"hevt_1"}"#,
        )
        .unwrap();
        assert_eq!(subscribe.run_id(), "run_ws");
        assert_eq!(subscribe.command_name(), "subscribe_events");
        assert!(!subscribe.requires_write_authorization());

        let err = HarnessControlCommand::parse_json(
            r#"{"command":"resolve_human_approval","run_id":" ","approval_id":"approval_1","decision":"approved"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("run_id must not be empty"));
    }

    #[tokio::test]
    async fn approval_control_command_creates_auditable_resolution_event() {
        let bus = HarnessEventBus::with_capacity(8);
        let command = HarnessControlCommand::parse_json(
            r#"{
                "command":"resolve_human_approval",
                "run_id":"run_approval_ws",
                "approval_id":"approval_deploy",
                "decision":"approved",
                "actor":"dashboard",
                "reason":"user clicked approve",
                "client_command_id":"cmd_1"
            }"#,
        )
        .unwrap();

        let event = bus.publish(harness_control_command_event_draft(&command, true));

        assert_eq!(event.kind, HarnessEventKind::HumanApprovalResolved);
        assert_eq!(event.level, HarnessEventLevel::Info);
        assert_eq!(event.run_id, "run_approval_ws");
        assert_eq!(event.payload["command"], "resolve_human_approval");
        assert_eq!(event.payload["approval_id"], "approval_deploy");
        assert_eq!(event.payload["decision"], "approved");
        assert_eq!(event.payload["approved"], true);
        assert_eq!(event.payload["actor"], "dashboard");
        assert_eq!(event.payload["reason_present"], true);
        assert_eq!(event.payload["client_command_id"], "cmd_1");
        assert_eq!(event.payload["status"], "resolved");
    }

    #[tokio::test]
    async fn unauthorized_control_command_is_rejected_and_redacted() {
        let bus = HarnessEventBus::with_capacity(8);
        let command = HarnessControlCommand::UiCommand {
            run_id: "run_control_reject".to_string(),
            name: "set_filter".to_string(),
            args: json!({"api_key": "sk-should-not-leak", "panel": "timeline"}),
            client_command_id: Some("cmd_reject".to_string()),
        };

        let event = bus.publish(harness_control_command_event_draft(&command, false));
        let serialized = serde_json::to_string(&event).unwrap();

        assert_eq!(event.kind, HarnessEventKind::ControlCommandRejected);
        assert_eq!(event.level, HarnessEventLevel::Warn);
        assert_eq!(event.payload["command"], "ui_command");
        assert_eq!(event.payload["status"], "rejected");
        assert_eq!(event.payload["authorized"], false);
        assert_eq!(event.payload["args"]["panel"], "timeline");
        assert_eq!(event.payload["args"]["api_key"], HARNESS_EVENT_REDACTED);
        assert!(!serialized.contains("sk-should-not-leak"));
    }

    #[test]
    fn publishing_without_subscribers_still_returns_event() {
        let bus = HarnessEventBus::with_capacity(8);

        let event = bus.publish(HarnessEventDraft::run_started("run_no_subscribers"));

        assert_eq!(event.sequence, 1);
        assert_eq!(event.kind, HarnessEventKind::RunStarted);
        assert!(event.event_id.starts_with("hevt_"));
    }

    #[test]
    fn broker_route_sanitizes_subjects_keys_and_consumers() {
        let event = HarnessEvent::new(
            "hevt_broker",
            "Run/Prod A",
            DateTime::parse_from_rfc3339("2026-05-08T05:21:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::ToolFinished,
            json!({"task_id": "Task:42", "status": "ok"}),
        )
        .with_session_id("Session 1");

        let route = harness_event_broker_route(&event);

        let run_token = encode_harness_event_broker_token("Run/Prod A");
        let session_token = encode_harness_event_broker_token("Session 1");
        let task_token = encode_harness_event_broker_token("Task:42");

        assert_eq!(route.run_id, run_token);
        assert_eq!(route.session_id.as_deref(), Some(session_token.as_str()));
        assert_eq!(route.task_id.as_deref(), Some(task_token.as_str()));
        assert_eq!(
            route.nats_subject,
            format!(
                "jcode.harness_events.v1.run.{run_token}.session.{session_token}.task.{task_token}"
            )
        );
        assert_eq!(
            route.redis_stream_key,
            format!(
                "jcode:harness-events:v1:run:{run_token}:events:session:{session_token}:task:{task_token}"
            )
        );
        assert_eq!(route.durable_consumer, format!("jcode-harness-{run_token}"));
        assert!(!route.nats_subject.contains("Run/Prod A"));
        assert!(!route.redis_stream_key.contains("Session 1"));
    }

    #[test]
    fn broker_token_encodes_untrusted_ids_without_wildcards_or_collisions() {
        let slash = encode_harness_event_broker_token("a/b");
        let underscore = encode_harness_event_broker_token("a_b");
        let wildcard = encode_harness_event_broker_token("run.*.>");

        assert_ne!(slash, underscore);
        assert!(wildcard.starts_with('b'));
        assert!(
            wildcard
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() || ch == 'b')
        );
        assert!(
            !wildcard
                .chars()
                .any(|ch| matches!(ch, '*' | '>' | '.' | ':' | '/' | ' '))
        );
        assert_eq!(encode_harness_event_broker_token(""), "b00");
    }

    #[test]
    fn broker_payload_round_trips_redacted_harness_event_json() {
        let event = HarnessEvent::new(
            "hevt_payload",
            "run_payload",
            DateTime::parse_from_rfc3339("2026-05-08T05:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            4,
            HarnessEventLevel::Info,
            HarnessEventKind::ToolFinished,
            json!({"status": "ok", "api_key": "sk-should-not-leak"}),
        );

        let payload = serialize_harness_event_broker_payload(&event).unwrap();
        let payload_text = String::from_utf8(payload.clone()).unwrap();
        let envelope = deserialize_harness_event_broker_payload(&payload).unwrap();

        assert!(!payload_text.contains("sk-should-not-leak"));
        assert_eq!(
            envelope.protocol_version,
            HARNESS_EVENT_BROKER_PROTOCOL_VERSION
        );
        assert_eq!(
            envelope.delivery_semantics,
            HARNESS_EVENT_BROKER_DELIVERY_SEMANTICS
        );
        assert_eq!(envelope.dedupe_key, "v1:hevt_payload");
        assert_eq!(envelope.event.payload["api_key"], HARNESS_EVENT_REDACTED);
        assert_eq!(envelope.event.event_id, event.event_id);
    }

    #[test]
    fn broker_delivery_and_ack_carry_dedupe_and_message_metadata() {
        let event = HarnessEvent::new(
            "hevt_delivery",
            "run delivery",
            DateTime::parse_from_rfc3339("2026-05-08T05:31:00Z")
                .unwrap()
                .with_timezone(&Utc),
            8,
            HarnessEventLevel::Info,
            HarnessEventKind::RunCompleted,
            json!({"status": "ok"}),
        );

        let delivery = harness_event_delivery(&event, Some("nats:42".to_string()), true, 0);
        let ack = harness_event_delivery_ack(
            &delivery,
            HarnessEventAckOutcome::Acked,
            Some("processed".to_string()),
        );

        assert_eq!(delivery.delivery_attempt, 1);
        assert!(delivery.redelivered);
        assert_eq!(delivery.envelope.dedupe_key, "v1:hevt_delivery");
        assert_eq!(delivery.message_id.as_deref(), Some("nats:42"));
        assert_eq!(ack.outcome, HarnessEventAckOutcome::Acked);
        assert_eq!(ack.dedupe_key, "v1:hevt_delivery");
        assert_eq!(ack.event_id, "hevt_delivery");
        assert_eq!(ack.message_id.as_deref(), Some("nats:42"));
        assert_eq!(ack.detail.as_deref(), Some("processed"));
    }

    #[test]
    fn ndjson_sink_and_source_traits_round_trip_after_last_event_id() {
        let temp = tempfile::Builder::new()
            .prefix("jcode-harness-events-sink-source-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("run_sink.ndjson");
        let mut sink = HarnessEventNdjsonSink::new(&path);
        let source = HarnessEventNdjsonSource::new(temp.path());
        let first = HarnessEvent::new(
            "hevt_first",
            "run_sink",
            DateTime::parse_from_rfc3339("2026-05-08T05:22:00Z")
                .unwrap()
                .with_timezone(&Utc),
            1,
            HarnessEventLevel::Info,
            HarnessEventKind::RunStarted,
            json!({"status": "started"}),
        );
        let second = HarnessEvent::new(
            "hevt_second",
            "run_sink",
            DateTime::parse_from_rfc3339("2026-05-08T05:22:01Z")
                .unwrap()
                .with_timezone(&Utc),
            2,
            HarnessEventLevel::Info,
            HarnessEventKind::RunCompleted,
            json!({"status": "ok"}),
        );

        let ack = sink.publish_event(&first).unwrap();
        sink.publish_event(&second).unwrap();
        sink.flush().unwrap();
        let tail = source
            .read_events_after("run_sink", Some("hevt_first"))
            .unwrap();

        assert_eq!(ack.sink, "ndjson");
        assert!(ack.durable);
        assert_eq!(ack.message_id.as_deref(), Some("run_sink:1"));
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].event_id, "hevt_second");
    }
}
