use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 2;
const DEFAULT_RETEST_DAYS: i64 = 14;
const LEDGER_ENV: &str = "JCODE_LIVE_TEST_LEDGER";
const COVERAGE_ENV: &str = "JCODE_LIVE_TEST_COVERAGE";

pub const CHECKPOINT_TAXONOMY_VERSION: u32 = 2;

pub mod checkpoints {
    pub const AUTH_UX_KEY_ENTRY: &str = "auth_ux_key_entry";
    pub const AUTH_CREDENTIAL_LOADED: &str = "auth_credential_loaded";
    pub const CREDENTIAL_PERSISTENCE: &str = "credential_persistence";
    pub const MODEL_CATALOG_LIVE_ENDPOINT: &str = "model_catalog_live_endpoint";
    pub const CATALOG_HOT_RELOAD_CURRENT_SESSION: &str = "catalog_hot_reload_current_session";
    pub const PICKER_LIVE_MODELS: &str = "picker_live_models";
    pub const PICKER_FALLBACK_LABELING: &str = "picker_fallback_labeling";
    pub const MODEL_SWITCH_ROUTE: &str = "model_switch_route";
    pub const NON_STREAMING_CHAT_COMPLETION: &str = "non_streaming_chat_completion";
    pub const STREAMING_CHAT_COMPLETION: &str = "streaming_chat_completion";
    pub const TOOL_CALL_PARSE: &str = "tool_call_parse";
    pub const TOOL_EXECUTION_LOOP: &str = "tool_execution_loop";
    pub const TOOL_RESULT_FOLLOWUP: &str = "tool_result_followup";
    pub const REAL_JCODE_TOOL_SMOKE: &str = "real_jcode_tool_smoke";
    pub const RESTART_PERSISTENCE: &str = "restart_persistence";
    pub const NEGATIVE_ERROR_UX: &str = "negative_error_ux";
    pub const MODEL_CAPABILITY_MATRIX: &str = "model_capability_matrix";
    pub const COST_QUOTA_SAFETY: &str = "cost_quota_safety";
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct LiveVerificationCheckpointDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub required_for_user_ready: bool,
    pub spends_balance: bool,
    pub description: &'static str,
}

const END_TO_END_CHECKPOINTS: &[LiveVerificationCheckpointDefinition] = &[
    LiveVerificationCheckpointDefinition {
        id: checkpoints::AUTH_UX_KEY_ENTRY,
        label: "Auth UX key entry",
        category: "auth",
        required_for_user_ready: true,
        spends_balance: false,
        description: "The user-facing auth path accepts a key, saves it, and does not crash or exit.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::AUTH_CREDENTIAL_LOADED,
        label: "Credential loaded",
        category: "auth",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Jcode can load the credential from the expected env/config source and records only a fingerprint.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::CREDENTIAL_PERSISTENCE,
        label: "Credential persistence",
        category: "auth",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Saved credentials persist in the expected config location and can be read back.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
        label: "Live model catalog endpoint",
        category: "catalog",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Authenticated GET /models succeeds and returns selectable chat models.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
        label: "Catalog hot reload in current session",
        category: "catalog",
        required_for_user_ready: true,
        spends_balance: false,
        description: "The active session refreshes provider routes immediately after auth changes.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::PICKER_LIVE_MODELS,
        label: "Picker shows live models",
        category: "picker",
        required_for_user_ready: true,
        spends_balance: false,
        description: "The model picker entries come from the live catalog and include the selected model.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::PICKER_FALLBACK_LABELING,
        label: "Picker fallback labeling",
        category: "picker",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Static or fallback routes are either absent from the picker or visibly labeled as fallback.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::MODEL_SWITCH_ROUTE,
        label: "Model switch route",
        category: "routing",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Switch requests route to the authenticated provider namespace and model API method.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::NON_STREAMING_CHAT_COMPLETION,
        label: "Non-streaming chat completion",
        category: "chat",
        required_for_user_ready: true,
        spends_balance: true,
        description: "POST /chat/completions returns an expected assistant response without streaming.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::STREAMING_CHAT_COMPLETION,
        label: "Streaming chat completion",
        category: "chat",
        required_for_user_ready: true,
        spends_balance: true,
        description: "Streaming deltas, finish reasons, and provider-specific chunks parse correctly.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::TOOL_CALL_PARSE,
        label: "Tool-call parse",
        category: "tools",
        required_for_user_ready: true,
        spends_balance: true,
        description: "The model emits a tool call whose name and arguments parse into a JSON object.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::TOOL_EXECUTION_LOOP,
        label: "Tool execution loop",
        category: "tools",
        required_for_user_ready: true,
        spends_balance: true,
        description: "A full Jcode turn executes a harmless local tool requested by the model.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::TOOL_RESULT_FOLLOWUP,
        label: "Tool-result followup",
        category: "tools",
        required_for_user_ready: true,
        spends_balance: true,
        description: "The provider accepts tool results and the model completes the final assistant response.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::REAL_JCODE_TOOL_SMOKE,
        label: "Real Jcode tool smoke",
        category: "tools",
        required_for_user_ready: true,
        spends_balance: true,
        description: "A normal Jcode agent turn uses the real streamed parser, advertised tool schema, registry execution, tool-result followup, and transcript validation without malformed tool calls.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::RESTART_PERSISTENCE,
        label: "Restart persistence",
        category: "persistence",
        required_for_user_ready: true,
        spends_balance: false,
        description: "After restart, credentials, catalog cache, and active model recover or safely auto-switch.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::NEGATIVE_ERROR_UX,
        label: "Negative/error UX",
        category: "errors",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Bad keys, no balance, rate limits, missing models, provider 5xx, and network blocks produce actionable errors, not crashes.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::MODEL_CAPABILITY_MATRIX,
        label: "Model capability matrix",
        category: "models",
        required_for_user_ready: true,
        spends_balance: true,
        description: "Each live catalog model has tracked chat, streaming, forced/unforced tool, and followup capability results.",
    },
    LiveVerificationCheckpointDefinition {
        id: checkpoints::COST_QUOTA_SAFETY,
        label: "Cost/quota safety",
        category: "safety",
        required_for_user_ready: true,
        spends_balance: false,
        description: "Usage, cost, retry-after, balance/rate-limit status, and retest timing are logged without leaking secrets.",
    },
];

pub fn end_to_end_checkpoint_definitions() -> &'static [LiveVerificationCheckpointDefinition] {
    END_TO_END_CHECKPOINTS
}

pub fn end_to_end_checkpoint_ids() -> impl Iterator<Item = &'static str> {
    END_TO_END_CHECKPOINTS
        .iter()
        .map(|definition| definition.id)
}

pub const STRICT_PROVIDER_MODEL_COVERAGE_CHECKPOINTS: &[&str] = &[
    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
    checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
    checkpoints::PICKER_LIVE_MODELS,
    checkpoints::PICKER_FALLBACK_LABELING,
    checkpoints::MODEL_SWITCH_ROUTE,
    checkpoints::NON_STREAMING_CHAT_COMPLETION,
    checkpoints::STREAMING_CHAT_COMPLETION,
    checkpoints::TOOL_CALL_PARSE,
    checkpoints::TOOL_EXECUTION_LOOP,
    checkpoints::TOOL_RESULT_FOLLOWUP,
    checkpoints::REAL_JCODE_TOOL_SMOKE,
];

pub fn strict_provider_model_coverage_checkpoint_ids() -> impl Iterator<Item = &'static str> {
    STRICT_PROVIDER_MODEL_COVERAGE_CHECKPOINTS.iter().copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IssueDrivenLiveProviderTarget {
    provider_id: &'static str,
    provider_label: &'static str,
    model: Option<&'static str>,
    reason: &'static str,
    issue_refs: &'static [&'static str],
}

const ISSUE_DRIVEN_LIVE_PROVIDER_TARGETS: &[IssueDrivenLiveProviderTarget] = &[
    IssueDrivenLiveProviderTarget {
        provider_id: "opencode-go",
        provider_label: "OpenCode Go",
        model: Some("kimi-k2.5"),
        reason: "OpenCode Go auth/server bootstrap regression and post-auth model routing",
        issue_refs: &["#234"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "nvidia-nim",
        provider_label: "NVIDIA NIM",
        model: Some("nvidia/llama-3.1-nemotron-ultra-253b-v1"),
        reason: "NVIDIA NIM provider auth and tool-smoke readiness",
        issue_refs: &["#164", "#197"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "ollama",
        provider_label: "Ollama",
        model: None,
        reason: "Ollama local/LAN setup, model catalog, and model switching regressions",
        issue_refs: &["#155", "#157"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "minimax",
        provider_label: "MiniMax",
        model: Some("MiniMax-M2.7"),
        reason: "MiniMax endpoint/key-region selection and live balance/readiness",
        issue_refs: &["#110", "#131", "#189"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "xiaomi-mimo",
        provider_label: "Xiaomi MiMo",
        model: Some("mimo-v2.5"),
        reason: "Xiaomi MiMo provider configuration and live model/tool support",
        issue_refs: &["#223"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "zai",
        provider_label: "Z.AI",
        model: Some("glm-4.5"),
        reason: "Z.AI and Zhipu BigModel regional endpoint compatibility",
        issue_refs: &["#156", "#161", "#177"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "bedrock",
        provider_label: "AWS Bedrock",
        model: None,
        reason: "AWS Bedrock bearer auth plus Application Inference Profile ARN support",
        issue_refs: &["#107", "#192"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "copilot",
        provider_label: "GitHub Copilot",
        model: Some("gpt-5.4"),
        reason: "Copilot GPT 5.4 model support and parameter compatibility",
        issue_refs: &["#190"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "gemini",
        provider_label: "Google Gemini",
        model: Some("gemini-2.5-pro"),
        reason: "Gemini catalog/picker regression and tool-call live readiness",
        issue_refs: &["#111", "#132"],
    },
    IssueDrivenLiveProviderTarget {
        provider_id: "openai-compatible",
        provider_label: "OpenAI-compatible",
        model: None,
        reason: "Generic OpenAI-compatible custom provider setup, default routing, and local endpoint support",
        issue_refs: &["#82", "#100", "#177", "#204"],
    },
];

pub fn checkpoint_catalog_metadata() -> Value {
    json!({
        "version": CHECKPOINT_TAXONOMY_VERSION,
        "checkpoints": END_TO_END_CHECKPOINTS,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveVerificationResult {
    Passed,
    Failed,
    Blocked,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveVerificationStageStatus {
    Passed,
    Failed,
    Blocked,
    Skipped,
    NotRun,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveVerificationStage {
    pub name: String,
    pub status: LiveVerificationStageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub evidence: Map<String, Value>,
}

impl LiveVerificationStage {
    pub fn new(name: impl Into<String>, status: LiveVerificationStageStatus) -> Self {
        Self {
            name: name.into(),
            status,
            duration_ms: None,
            evidence: Map::new(),
        }
    }

    pub fn passed(name: impl Into<String>) -> Self {
        Self::new(name, LiveVerificationStageStatus::Passed)
    }

    pub fn failed(name: impl Into<String>, error: impl Into<String>) -> Self {
        Self::new(name, LiveVerificationStageStatus::Failed).with_evidence(
            "error",
            Value::String(redact_secret_like_text(&error.into())),
        )
    }

    pub fn blocked(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(name, LiveVerificationStageStatus::Blocked).with_evidence(
            "reason",
            Value::String(redact_secret_like_text(&reason.into())),
        )
    }

    pub fn skipped(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(name, LiveVerificationStageStatus::Skipped).with_evidence(
            "reason",
            Value::String(redact_secret_like_text(&reason.into())),
        )
    }

    pub fn not_run(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(name, LiveVerificationStageStatus::NotRun).with_evidence(
            "reason",
            Value::String(redact_secret_like_text(&reason.into())),
        )
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_evidence(mut self, key: impl Into<String>, value: Value) -> Self {
        self.evidence.insert(key.into(), sanitize_json_value(value));
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveVerificationAuth {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

impl LiveVerificationAuth {
    pub fn from_secret(
        source: impl Into<String>,
        env_key: Option<impl Into<String>>,
        secret: &str,
    ) -> Self {
        Self {
            source: source.into(),
            env_key: env_key.map(Into::into),
            fingerprint: fingerprint_secret(secret),
        }
    }

    pub fn non_secret(source: impl Into<String>, env_key: Option<impl Into<String>>) -> Self {
        Self {
            source: source.into(),
            env_key: env_key.map(Into::into),
            fingerprint: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveVerificationBuild {
    pub jcode_version: String,
    pub jcode_git_hash: String,
    pub jcode_git_date: String,
    pub jcode_git_dirty: bool,
    pub jcode_semver: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
}

impl LiveVerificationBuild {
    pub fn current() -> Self {
        let version = env!("JCODE_VERSION").to_string();
        Self {
            jcode_git_dirty: version.contains("dirty"),
            jcode_version: version,
            jcode_git_hash: env!("JCODE_GIT_HASH").to_string(),
            jcode_git_date: env!("JCODE_GIT_DATE").to_string(),
            jcode_semver: env!("JCODE_SEMVER").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            pid: std::process::id(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveVerificationEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub recorded_at: DateTime<Utc>,
    pub test_name: String,
    pub provider_id: String,
    pub provider_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_checkpoints: Vec<String>,
    pub result: LiveVerificationResult,
    pub auth: LiveVerificationAuth,
    pub build: LiveVerificationBuild,
    pub retest_after: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<LiveVerificationStage>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl LiveVerificationEvent {
    pub fn new(
        test_name: impl Into<String>,
        provider_id: impl Into<String>,
        provider_label: impl Into<String>,
        auth: LiveVerificationAuth,
        result: LiveVerificationResult,
    ) -> Self {
        let recorded_at = Utc::now();
        let test_name = test_name.into();
        let provider_id = provider_id.into();
        let provider_label = provider_label.into();
        let event_id = event_id(&recorded_at, &test_name, &provider_id);
        Self {
            schema_version: SCHEMA_VERSION,
            event_id,
            recorded_at,
            test_name,
            provider_id,
            provider_label,
            endpoint: None,
            model: None,
            capabilities: Vec::new(),
            expected_checkpoints: Vec::new(),
            result,
            auth,
            build: LiveVerificationBuild::current(),
            retest_after: recorded_at + Duration::days(DEFAULT_RETEST_DAYS),
            stages: Vec::new(),
            metadata: Map::new(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_capabilities<I, S>(mut self, capabilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.capabilities = capabilities.into_iter().map(Into::into).collect();
        self.capabilities.sort();
        self.capabilities.dedup();
        self
    }

    pub fn with_retest_days(mut self, days: i64) -> Self {
        self.retest_after = self.recorded_at + Duration::days(days.max(1));
        self
    }

    pub fn with_expected_checkpoints<I, S>(mut self, checkpoints: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.expected_checkpoints = dedup_preserving_order(checkpoints.into_iter().map(Into::into));
        self
    }

    pub fn with_standard_end_to_end_checkpoints(self) -> Self {
        self.with_expected_checkpoints(end_to_end_checkpoint_ids())
            .with_metadata(
                "checkpoint_taxonomy_version",
                json!(CHECKPOINT_TAXONOMY_VERSION),
            )
    }

    pub fn with_stage(mut self, stage: LiveVerificationStage) -> Self {
        self.stages.push(stage);
        self
    }

    pub fn with_stages<I>(mut self, stages: I) -> Self
    where
        I: IntoIterator<Item = LiveVerificationStage>,
    {
        self.stages.extend(stages);
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), sanitize_json_value(value));
        self
    }

    pub fn checkpoint_statuses(&self) -> BTreeMap<String, LiveVerificationStageStatus> {
        let mut statuses = BTreeMap::new();
        for checkpoint in &self.expected_checkpoints {
            statuses.insert(checkpoint.clone(), LiveVerificationStageStatus::NotRun);
        }
        for stage in &self.stages {
            statuses.insert(stage.name.clone(), stage.status.clone());
        }
        statuses
    }

    pub fn readiness_gaps(&self) -> Vec<String> {
        let statuses = self.checkpoint_statuses();
        END_TO_END_CHECKPOINTS
            .iter()
            .filter(|definition| definition.required_for_user_ready)
            .filter(|definition| {
                self.expected_checkpoints
                    .iter()
                    .any(|checkpoint| checkpoint == definition.id)
            })
            .filter(|definition| {
                statuses.get(definition.id) != Some(&LiveVerificationStageStatus::Passed)
            })
            .map(|definition| definition.id.to_string())
            .collect()
    }

    pub fn user_ready(&self) -> bool {
        self.result == LiveVerificationResult::Passed && self.readiness_gaps().is_empty()
    }

    pub fn with_not_run_for_missing_expected_checkpoints(mut self, reason: &str) -> Self {
        let covered = self
            .stages
            .iter()
            .map(|stage| stage.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let missing = self
            .expected_checkpoints
            .iter()
            .filter(|checkpoint| !covered.contains(checkpoint.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for checkpoint in missing {
            self.stages
                .push(LiveVerificationStage::not_run(checkpoint, reason));
        }
        self
    }

    pub fn coverage_key(&self) -> String {
        let model = self.model.as_deref().unwrap_or("*");
        let capabilities = if self.capabilities.is_empty() {
            "unspecified".to_string()
        } else {
            self.capabilities.join("+")
        };
        format!("{}::{model}::{capabilities}", self.provider_id)
    }
}

fn dedup_preserving_order(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut deduped = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            deduped.push(item);
        }
    }
    deduped
}

pub fn append_event(event: &LiveVerificationEvent) -> Result<LiveVerificationPaths> {
    let paths = LiveVerificationPaths::resolve()?;
    append_event_to_paths(event, &paths)?;
    Ok(paths)
}

fn append_event_to_paths(
    event: &LiveVerificationEvent,
    paths: &LiveVerificationPaths,
) -> Result<()> {
    if let Some(parent) = paths.events.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create live test ledger dir {}", parent.display()))?;
    }
    let line = serde_json::to_string(event).context("serialize live verification event")?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.events)
        .with_context(|| format!("open live test ledger {}", paths.events.display()))?;
    writeln!(file, "{line}").context("append live verification event")?;
    update_coverage(event, &paths.coverage)?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveVerificationPaths {
    pub events: PathBuf,
    pub coverage: PathBuf,
}

impl LiveVerificationPaths {
    pub fn resolve() -> Result<Self> {
        let events = std::env::var(LEDGER_ENV)
            .ok()
            .map(PathBuf::from)
            .unwrap_or(crate::storage::app_config_dir()?.join("live-tests/events.jsonl"));
        let coverage = std::env::var(COVERAGE_ENV)
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                events
                    .parent()
                    .map(|parent| parent.join("coverage.json"))
                    .unwrap_or_else(|| PathBuf::from("coverage.json"))
            });
        Ok(Self { events, coverage })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveVerificationCoverage {
    pub schema_version: u32,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub checkpoint_taxonomy_version: u32,
    #[serde(default)]
    pub checkpoint_taxonomy: Value,
    #[serde(default)]
    pub latest: BTreeMap<String, LiveVerificationCoverageEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveVerificationCoverageEntry {
    pub event_id: String,
    pub recorded_at: DateTime<Utc>,
    pub test_name: String,
    pub provider_id: String,
    pub provider_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_checkpoints: Vec<String>,
    pub result: LiveVerificationResult,
    pub retest_after: DateTime<Utc>,
    pub jcode_version: String,
    pub jcode_git_hash: String,
    pub jcode_git_dirty: bool,
    #[serde(default)]
    pub user_ready: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_gaps: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checkpoint_statuses: BTreeMap<String, LiveVerificationStageStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stage_statuses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveCoverageCheckpointDefinition {
    pub id: String,
    pub label: String,
    pub category: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveProviderModelCoveragePair {
    pub provider_id: String,
    pub provider_label: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_provider_ids: Vec<String>,
    pub covered: bool,
    pub latest_recorded_at: DateTime<Utc>,
    pub entries: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passed_checkpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_checkpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub non_passing_checkpoints: BTreeMap<String, LiveVerificationStageStatus>,
}

impl LiveProviderModelCoveragePair {
    fn checkpoint_status(&self, checkpoint: &str) -> Option<LiveVerificationStageStatus> {
        if self
            .passed_checkpoints
            .iter()
            .any(|passed| passed == checkpoint)
        {
            Some(LiveVerificationStageStatus::Passed)
        } else if let Some(status) = self.non_passing_checkpoints.get(checkpoint) {
            Some(status.clone())
        } else if self
            .missing_checkpoints
            .iter()
            .any(|missing| missing == checkpoint)
        {
            None
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveProviderCoverageSummary {
    pub provider_id: String,
    pub provider_label: String,
    pub total_model_pairs: usize,
    pub covered_model_pairs: usize,
    pub coverage_percent: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models_without_strict_coverage: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IssueDrivenLiveProviderTargetSummary {
    pub provider_id: String,
    pub provider_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub status: String,
    pub reason: String,
    pub issue_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered_models: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub missing_checkpoints_by_model: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveProviderModelCoverageSummary {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub coverage_source: String,
    pub denominator: String,
    pub covered_definition: String,
    pub total_provider_model_pairs: usize,
    pub covered_provider_model_pairs: usize,
    pub coverage_percent: f64,
    pub required_checkpoints: Vec<LiveCoverageCheckpointDefinition>,
    pub providers: Vec<LiveProviderCoverageSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered_pairs: Vec<LiveProviderModelCoveragePair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered_pairs: Vec<LiveProviderModelCoveragePair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_only_entries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_provider_ids_without_live_model_coverage: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issue_driven_targets: Vec<IssueDrivenLiveProviderTargetSummary>,
}

fn update_coverage(event: &LiveVerificationEvent, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create live test coverage dir {}", parent.display()))?;
    }
    let mut coverage = if path.exists() {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read live test coverage {}", path.display()))?;
        serde_json::from_str::<LiveVerificationCoverage>(&raw).unwrap_or_else(|_| {
            LiveVerificationCoverage {
                schema_version: SCHEMA_VERSION,
                updated_at: Utc::now(),
                checkpoint_taxonomy_version: CHECKPOINT_TAXONOMY_VERSION,
                checkpoint_taxonomy: checkpoint_catalog_metadata(),
                latest: BTreeMap::new(),
            }
        })
    } else {
        LiveVerificationCoverage {
            schema_version: SCHEMA_VERSION,
            updated_at: Utc::now(),
            checkpoint_taxonomy_version: CHECKPOINT_TAXONOMY_VERSION,
            checkpoint_taxonomy: checkpoint_catalog_metadata(),
            latest: BTreeMap::new(),
        }
    };

    coverage.schema_version = SCHEMA_VERSION;
    coverage.updated_at = Utc::now();
    coverage.checkpoint_taxonomy_version = CHECKPOINT_TAXONOMY_VERSION;
    coverage.checkpoint_taxonomy = checkpoint_catalog_metadata();
    let checkpoint_statuses = event.checkpoint_statuses();
    let readiness_gaps = event.readiness_gaps();
    coverage.latest.insert(
        event.coverage_key(),
        LiveVerificationCoverageEntry {
            event_id: event.event_id.clone(),
            recorded_at: event.recorded_at,
            test_name: event.test_name.clone(),
            provider_id: event.provider_id.clone(),
            provider_label: event.provider_label.clone(),
            model: event.model.clone(),
            capabilities: event.capabilities.clone(),
            expected_checkpoints: event.expected_checkpoints.clone(),
            result: event.result.clone(),
            retest_after: event.retest_after,
            jcode_version: event.build.jcode_version.clone(),
            jcode_git_hash: event.build.jcode_git_hash.clone(),
            jcode_git_dirty: event.build.jcode_git_dirty,
            user_ready: event.user_ready(),
            readiness_gaps,
            checkpoint_statuses,
            stage_statuses: event
                .stages
                .iter()
                .map(|stage| format!("{}:{:?}", stage.name, stage.status))
                .collect(),
        },
    );
    let serialized = serde_json::to_string_pretty(&coverage)
        .context("serialize live verification coverage summary")?;
    std::fs::write(path, serialized)
        .with_context(|| format!("write live test coverage {}", path.display()))?;
    Ok(())
}

pub fn load_coverage(coverage_path: Option<&Path>) -> Result<(LiveVerificationCoverage, PathBuf)> {
    let path = if let Some(path) = coverage_path {
        path.to_path_buf()
    } else {
        LiveVerificationPaths::resolve()?.coverage
    };
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read live test coverage {}", path.display()))?;
    let coverage = serde_json::from_str::<LiveVerificationCoverage>(&raw)
        .with_context(|| format!("parse live test coverage {}", path.display()))?;
    Ok((coverage, path))
}

pub fn format_provider_test_coverage_report(
    provider_query: &str,
    model_query: &str,
    coverage_path: Option<&Path>,
) -> String {
    let mut out = String::new();
    out.push_str("# Provider test coverage\n\n");
    out.push_str("Developer/live verification evidence recorded by jcode. This is evidence, not a guarantee of future provider availability.\n\n");
    out.push_str(&format!("Provider: `{}`\n", provider_query));
    out.push_str(&format!("Model: `{}`\n\n", model_query));

    let (coverage, path) = match load_coverage(coverage_path) {
        Ok(loaded) => loaded,
        Err(err) => {
            out.push_str("Status: **No verification ledger found on this install**\n\n");
            out.push_str("No local or bundled developer live-test coverage file could be loaded. ");
            out.push_str("Once jcode ships a curated developer coverage snapshot, this command should prefer that snapshot and separately show local evidence.\n\n");
            out.push_str(&format!("Ledger error: `{}`\n\n", err));
            out.push_str("You can generate local evidence with:\n\n");
            out.push_str(&format!(
                "```bash\njcode auth-test --provider {} --model {}\n```",
                provider_query, model_query
            ));
            return out;
        }
    };

    let provider_norm = normalize_provider_test_coverage_key(provider_query);
    let provider_aliases = provider_test_coverage_lookup_aliases(&provider_norm);
    let model_norm = normalize_provider_test_coverage_key(model_query);
    let mut matches = coverage
        .latest
        .values()
        .filter(|entry| {
            let entry_provider = normalize_provider_test_coverage_key(&entry.provider_id);
            let entry_label = normalize_provider_test_coverage_key(&entry.provider_label);
            let entry_model = entry
                .model
                .as_deref()
                .map(normalize_provider_test_coverage_key)
                .unwrap_or_else(|| "*".to_string());
            (provider_aliases.contains(&entry_provider) || provider_aliases.contains(&entry_label))
                && (entry_model == model_norm || model_norm == "*")
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|entry| entry.recorded_at);
    let mut latest_by_target = BTreeMap::new();
    for entry in matches {
        let key = (
            normalize_provider_test_coverage_key(&entry.provider_id),
            entry
                .model
                .as_deref()
                .map(normalize_provider_test_coverage_key)
                .unwrap_or_else(|| "*".to_string()),
            entry.test_name.clone(),
        );
        latest_by_target.insert(key, entry);
    }
    let mut matches = latest_by_target.into_values().collect::<Vec<_>>();
    matches.sort_by_key(|entry| entry.recorded_at);

    let Some(entry) = matches.last() else {
        out.push_str("Status: **Not yet covered by this jcode verification ledger**\n\n");
        out.push_str(&format!("Ledger: `{}`\n\n", path.display()));
        out.push_str("This does not mean the provider/model is broken. It only means jcode has no recorded live verification evidence for this exact provider/model pair.\n");
        return out;
    };

    let mut expected_checkpoints = Vec::new();
    let mut checkpoint_statuses = BTreeMap::new();
    for matched in &matches {
        for checkpoint in &matched.expected_checkpoints {
            if !expected_checkpoints.contains(checkpoint) {
                expected_checkpoints.push(checkpoint.clone());
            }
        }
        for (checkpoint, status) in &matched.checkpoint_statuses {
            let merged = merge_checkpoint_status(checkpoint_statuses.get(checkpoint), status);
            checkpoint_statuses.insert(checkpoint.clone(), merged);
        }
    }

    let passed = checkpoint_statuses
        .values()
        .filter(|status| matches!(status, LiveVerificationStageStatus::Passed))
        .count();
    let total = checkpoint_statuses.len().max(expected_checkpoints.len());
    let all_expected_passed = expected_checkpoints.iter().all(|checkpoint| {
        matches!(
            checkpoint_statuses.get(checkpoint),
            Some(LiveVerificationStageStatus::Passed)
        )
    });
    let status = if all_expected_passed && total > 0 {
        "Fully tested"
    } else if passed > 0 {
        "Partially tested"
    } else {
        "Tested, but no passing checkpoints recorded"
    };

    out.push_str(&format!("Status: **{}**\n", status));
    out.push_str(&format!("Last tested: `{}`\n", entry.recorded_at));
    out.push_str(&format!("Evidence source: `{}`\n", path.display()));
    out.push_str(&format!("Matching evidence entries: `{}`\n", matches.len()));
    out.push_str(&format!("Test name: `{}`\n", entry.test_name));
    out.push_str(&format!(
        "Tested with: `jcode {}` ({}){}\n\n",
        entry.jcode_version,
        entry.jcode_git_hash,
        if entry.jcode_git_dirty { ", dirty" } else { "" }
    ));

    out.push_str("## Checkpoints\n\n");
    for checkpoint in &expected_checkpoints {
        let status = checkpoint_statuses
            .get(checkpoint)
            .cloned()
            .unwrap_or(LiveVerificationStageStatus::NotRun);
        out.push_str(&format!(
            "{} {} - `{:?}`\n",
            provider_test_coverage_icon(&status),
            provider_test_coverage_checkpoint_label(checkpoint),
            status
        ));
    }

    let readiness_gaps = expected_checkpoints
        .iter()
        .filter(|checkpoint| {
            !matches!(
                checkpoint_statuses.get(*checkpoint),
                Some(LiveVerificationStageStatus::Passed)
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !readiness_gaps.is_empty() {
        out.push_str("\n## Readiness gaps\n\n");
        for gap in &readiness_gaps {
            out.push_str(&format!("- {}\n", gap));
        }
    }

    out.push_str("\n## What this means\n\n");
    out.push_str("These checks exercise real jcode runtime paths, including basic chat and tool-use smoke tests when present. Missing evidence should be read as 'not yet recorded', not as a failure.\n");
    out
}

fn normalize_provider_test_coverage_key(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

fn provider_test_coverage_lookup_aliases(provider_norm: &str) -> Vec<String> {
    let mut aliases = vec![provider_norm.to_string()];
    match provider_norm {
        "opencode" => aliases.push("opencode-zen".to_string()),
        "opencode-zen" => aliases.push("opencode".to_string()),
        _ => {}
    }
    aliases
}

fn provider_test_coverage_icon(status: &LiveVerificationStageStatus) -> &'static str {
    match status {
        LiveVerificationStageStatus::Passed => "✓",
        LiveVerificationStageStatus::Failed => "✗",
        LiveVerificationStageStatus::Blocked => "!",
        LiveVerificationStageStatus::Skipped => "-",
        LiveVerificationStageStatus::NotRun => "•",
    }
}

fn provider_test_coverage_checkpoint_label(checkpoint: &str) -> String {
    match checkpoint {
        checkpoints::AUTH_CREDENTIAL_LOADED => "Credential loaded".to_string(),
        checkpoints::NON_STREAMING_CHAT_COMPLETION => "Basic chat completion".to_string(),
        checkpoints::TOOL_CALL_PARSE => "Tool call parsed".to_string(),
        checkpoints::TOOL_EXECUTION_LOOP => "Tool execution loop".to_string(),
        checkpoints::TOOL_RESULT_FOLLOWUP => "Tool result follow-up".to_string(),
        checkpoints::REAL_JCODE_TOOL_SMOKE => "Real jcode tool smoke".to_string(),
        other => other.replace('_', " "),
    }
}

#[derive(Default)]
struct ProviderModelCoverageBuilder {
    provider_id: String,
    provider_label: String,
    model: String,
    latest_recorded_at: Option<DateTime<Utc>>,
    entries: usize,
    capabilities: BTreeSet<String>,
    source_provider_ids: BTreeSet<String>,
    checkpoint_statuses: BTreeMap<String, LiveVerificationStageStatus>,
}

impl ProviderModelCoverageBuilder {
    fn record(&mut self, entry: &LiveVerificationCoverageEntry) {
        self.entries += 1;
        if self.provider_id.is_empty() {
            self.provider_id = entry.provider_id.clone();
        }
        if self.provider_label.is_empty() {
            self.provider_label = entry.provider_label.clone();
        }
        if self.model.is_empty() {
            self.model = entry.model.clone().unwrap_or_else(|| "*".to_string());
        }
        self.source_provider_ids.insert(entry.provider_id.clone());
        if self
            .latest_recorded_at
            .map(|latest| entry.recorded_at > latest)
            .unwrap_or(true)
        {
            self.latest_recorded_at = Some(entry.recorded_at);
        }
        self.capabilities.extend(entry.capabilities.iter().cloned());
        for (checkpoint, status) in &entry.checkpoint_statuses {
            let merged = merge_checkpoint_status(self.checkpoint_statuses.get(checkpoint), status);
            self.checkpoint_statuses.insert(checkpoint.clone(), merged);
        }
    }

    fn finish(self) -> LiveProviderModelCoveragePair {
        let mut passed_checkpoints = Vec::new();
        let mut missing_checkpoints = Vec::new();
        let mut non_passing_checkpoints = BTreeMap::new();

        for checkpoint in STRICT_PROVIDER_MODEL_COVERAGE_CHECKPOINTS {
            match self.checkpoint_statuses.get(*checkpoint) {
                Some(LiveVerificationStageStatus::Passed) => {
                    passed_checkpoints.push((*checkpoint).to_string());
                }
                Some(status) => {
                    non_passing_checkpoints.insert((*checkpoint).to_string(), status.clone());
                }
                None => missing_checkpoints.push((*checkpoint).to_string()),
            }
        }

        let covered = missing_checkpoints.is_empty() && non_passing_checkpoints.is_empty();
        LiveProviderModelCoveragePair {
            provider_id: self.provider_id,
            provider_label: self.provider_label,
            model: self.model,
            source_provider_ids: self.source_provider_ids.into_iter().collect(),
            covered,
            latest_recorded_at: self.latest_recorded_at.unwrap_or_else(Utc::now),
            entries: self.entries,
            capabilities: self.capabilities.into_iter().collect(),
            passed_checkpoints,
            missing_checkpoints,
            non_passing_checkpoints,
        }
    }
}

fn merge_checkpoint_status(
    current: Option<&LiveVerificationStageStatus>,
    incoming: &LiveVerificationStageStatus,
) -> LiveVerificationStageStatus {
    use LiveVerificationStageStatus::{Blocked, Failed, NotRun, Passed, Skipped};

    fn rank(status: &LiveVerificationStageStatus) -> u8 {
        match status {
            Passed => 5,
            Failed => 4,
            Blocked => 3,
            Skipped => 2,
            NotRun => 1,
        }
    }

    match current {
        Some(existing) if rank(existing) >= rank(incoming) => existing.clone(),
        _ => incoming.clone(),
    }
}

pub fn strict_live_provider_model_coverage_summary(
    coverage: &LiveVerificationCoverage,
    coverage_source: impl Into<String>,
) -> LiveProviderModelCoverageSummary {
    let mut builders: BTreeMap<(String, String), ProviderModelCoverageBuilder> = BTreeMap::new();
    let mut provider_only_entries = BTreeSet::new();

    for (key, entry) in &coverage.latest {
        let model = entry.model.as_deref().map(str::trim).unwrap_or("*");
        if model.is_empty() || model == "*" {
            provider_only_entries.insert(key.clone());
            continue;
        }
        let provider_identity =
            canonical_live_provider_identity(&entry.provider_id, &entry.provider_label);
        builders
            .entry((provider_identity.0.clone(), model.to_string()))
            .or_insert_with(|| ProviderModelCoverageBuilder {
                provider_id: provider_identity.0.clone(),
                provider_label: provider_identity.1.clone(),
                model: model.to_string(),
                ..ProviderModelCoverageBuilder::default()
            })
            .record(entry);
    }

    let mut covered_pairs = Vec::new();
    let mut uncovered_pairs = Vec::new();
    let mut provider_labels = BTreeMap::new();
    let mut provider_totals: BTreeMap<String, (usize, usize, Vec<String>)> = BTreeMap::new();

    for pair in builders
        .into_values()
        .map(ProviderModelCoverageBuilder::finish)
    {
        provider_labels.insert(pair.provider_id.clone(), pair.provider_label.clone());
        let totals = provider_totals
            .entry(pair.provider_id.clone())
            .or_insert_with(|| (0, 0, Vec::new()));
        totals.0 += 1;
        if pair.covered {
            totals.1 += 1;
            covered_pairs.push(pair);
        } else {
            totals.2.push(pair.model.clone());
            uncovered_pairs.push(pair);
        }
    }

    covered_pairs.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| a.model.cmp(&b.model))
    });
    uncovered_pairs.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| a.model.cmp(&b.model))
    });

    let providers = provider_totals
        .into_iter()
        .map(
            |(provider_id, (total, covered, mut models_without_strict_coverage))| {
                models_without_strict_coverage.sort();
                LiveProviderCoverageSummary {
                    provider_label: provider_labels
                        .get(&provider_id)
                        .cloned()
                        .unwrap_or_else(|| provider_id.clone()),
                    provider_id,
                    total_model_pairs: total,
                    covered_model_pairs: covered,
                    coverage_percent: percent(covered, total),
                    models_without_strict_coverage,
                }
            },
        )
        .collect::<Vec<_>>();

    let observed_providers = providers
        .iter()
        .map(|provider| provider.provider_id.as_str())
        .collect::<BTreeSet<_>>();
    let known_provider_ids_without_live_model_coverage = known_live_model_provider_ids()
        .into_iter()
        .filter(|provider_id| !observed_providers.contains(provider_id.as_str()))
        .collect::<Vec<_>>();

    let total_provider_model_pairs = covered_pairs.len() + uncovered_pairs.len();
    let covered_provider_model_pairs = covered_pairs.len();
    let issue_driven_targets = issue_driven_target_summaries(&covered_pairs, &uncovered_pairs);
    LiveProviderModelCoverageSummary {
        schema_version: SCHEMA_VERSION,
        generated_at: Utc::now(),
        coverage_source: coverage_source.into(),
        denominator: "observed canonical provider/model pairs in the live verification coverage ledger"
            .to_string(),
        covered_definition: "covered means every strict provider/model E2E checkpoint passed for the exact canonical provider id and model after login-provider alias normalization: live catalog, current-session catalog refresh, TUI picker visibility/fallback labeling, model switch route, non-streaming completion, streaming completion, tool-call parse, tool execution loop, tool-result followup, and real Jcode tool smoke".to_string(),
        total_provider_model_pairs,
        covered_provider_model_pairs,
        coverage_percent: percent(covered_provider_model_pairs, total_provider_model_pairs),
        required_checkpoints: strict_required_checkpoint_definitions(),
        providers,
        uncovered_pairs,
        covered_pairs,
        provider_only_entries: provider_only_entries.into_iter().collect(),
        known_provider_ids_without_live_model_coverage,
        issue_driven_targets,
    }
}

fn percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        ((numerator as f64 / denominator as f64) * 10_000.0).round() / 100.0
    }
}

fn strict_required_checkpoint_definitions() -> Vec<LiveCoverageCheckpointDefinition> {
    STRICT_PROVIDER_MODEL_COVERAGE_CHECKPOINTS
        .iter()
        .map(|id| {
            let definition = END_TO_END_CHECKPOINTS
                .iter()
                .find(|definition| definition.id == *id)
                .expect("strict coverage checkpoint should exist in taxonomy");
            LiveCoverageCheckpointDefinition {
                id: definition.id.to_string(),
                label: definition.label.to_string(),
                category: definition.category.to_string(),
            }
        })
        .collect()
}

fn issue_driven_target_summaries(
    covered_pairs: &[LiveProviderModelCoveragePair],
    uncovered_pairs: &[LiveProviderModelCoveragePair],
) -> Vec<IssueDrivenLiveProviderTargetSummary> {
    ISSUE_DRIVEN_LIVE_PROVIDER_TARGETS
        .iter()
        .map(|target| issue_driven_target_summary(target, covered_pairs, uncovered_pairs))
        .collect()
}

fn issue_driven_target_summary(
    target: &IssueDrivenLiveProviderTarget,
    covered_pairs: &[LiveProviderModelCoveragePair],
    uncovered_pairs: &[LiveProviderModelCoveragePair],
) -> IssueDrivenLiveProviderTargetSummary {
    let matches_target = |pair: &LiveProviderModelCoveragePair| {
        pair.provider_id == target.provider_id
            && target
                .model
                .map(|model| pair.model == model)
                .unwrap_or(true)
    };

    let mut observed_models = covered_pairs
        .iter()
        .chain(uncovered_pairs.iter())
        .filter(|pair| matches_target(pair))
        .map(|pair| pair.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    observed_models.sort();

    let mut covered_models = covered_pairs
        .iter()
        .filter(|pair| matches_target(pair))
        .map(|pair| pair.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    covered_models.sort();

    let missing_checkpoints_by_model = uncovered_pairs
        .iter()
        .filter(|pair| matches_target(pair))
        .map(|pair| {
            let mut missing = pair.missing_checkpoints.clone();
            missing.extend(
                pair.non_passing_checkpoints
                    .iter()
                    .map(|(checkpoint, status)| format!("{checkpoint}:{status:?}")),
            );
            (pair.model.clone(), missing)
        })
        .collect::<BTreeMap<_, _>>();

    let status = if !covered_models.is_empty() {
        "strict_covered"
    } else if !observed_models.is_empty() {
        "observed_missing_strict_checkpoints"
    } else {
        "no_model_specific_live_evidence"
    };

    IssueDrivenLiveProviderTargetSummary {
        provider_id: target.provider_id.to_string(),
        provider_label: target.provider_label.to_string(),
        model: target.model.map(ToString::to_string),
        status: status.to_string(),
        reason: target.reason.to_string(),
        issue_refs: target
            .issue_refs
            .iter()
            .map(|issue| (*issue).to_string())
            .collect(),
        observed_models,
        covered_models,
        missing_checkpoints_by_model,
    }
}

fn canonical_live_provider_identity(provider_id: &str, provider_label: &str) -> (String, String) {
    let trimmed = provider_id.trim();
    if let Some(provider) = crate::provider_catalog::resolve_login_provider(trimmed) {
        return (provider.id.to_string(), provider.display_name.to_string());
    }

    let id = if trimmed.is_empty() {
        provider_label.trim().to_ascii_lowercase()
    } else {
        trimmed.to_ascii_lowercase()
    };
    let label = if provider_label.trim().is_empty() {
        id.clone()
    } else {
        provider_label.to_string()
    };
    (id, label)
}

fn known_live_model_provider_ids() -> Vec<String> {
    let mut ids = BTreeSet::new();
    for provider in crate::provider_catalog::login_providers() {
        if !matches!(
            provider.target,
            crate::provider_catalog::LoginProviderTarget::AutoImport
                | crate::provider_catalog::LoginProviderTarget::Google
        ) {
            ids.insert(provider.id.to_string());
        }
    }
    ids.into_iter().collect()
}

pub fn format_strict_live_provider_model_coverage_summary(
    summary: &LiveProviderModelCoverageSummary,
    gap_limit: usize,
) -> String {
    let mut out = String::new();
    out.push_str("Live provider/model E2E coverage\n");
    out.push_str(&format!("Source: {}\n", summary.coverage_source));
    out.push_str(&format!("Denominator: {}\n", summary.denominator));
    out.push_str(&format!(
        "Covered definition: {}\n",
        summary.covered_definition
    ));
    out.push_str(&format!(
        "Coverage: {}/{} provider/model pairs ({:.2}%)\n\n",
        summary.covered_provider_model_pairs,
        summary.total_provider_model_pairs,
        summary.coverage_percent
    ));

    let mut basic_chat_passed = 0usize;
    let mut tool_smoke_passed = 0usize;
    let mut tool_smoke_skipped = 0usize;
    let all_pairs = summary
        .covered_pairs
        .iter()
        .chain(summary.uncovered_pairs.iter())
        .collect::<Vec<_>>();
    for pair in &all_pairs {
        if matches!(
            pair.checkpoint_status(crate::live_tests::checkpoints::NON_STREAMING_CHAT_COMPLETION),
            Some(LiveVerificationStageStatus::Passed)
        ) {
            basic_chat_passed += 1;
        }
        match pair.checkpoint_status(crate::live_tests::checkpoints::REAL_JCODE_TOOL_SMOKE) {
            Some(LiveVerificationStageStatus::Passed) => tool_smoke_passed += 1,
            Some(LiveVerificationStageStatus::Skipped) => tool_smoke_skipped += 1,
            _ => {}
        }
    }
    if !all_pairs.is_empty() {
        out.push_str(&format!(
            "Auth-test smoke snapshot: basic chat passed {}/{} observed provider/model pairs; real tool smoke passed {}/{} and skipped {}.\n",
            basic_chat_passed,
            all_pairs.len(),
            tool_smoke_passed,
            all_pairs.len(),
            tool_smoke_skipped
        ));
        out.push_str("Note: this is separate from strict E2E coverage, which also requires catalog, TUI picker, model-switch, and streaming checkpoints.\n\n");
    }

    out.push_str("Required checkpoints:\n");
    for checkpoint in &summary.required_checkpoints {
        out.push_str(&format!("  - {} ({})\n", checkpoint.id, checkpoint.label));
    }

    out.push_str("\nProviders:\n");
    if summary.providers.is_empty() {
        out.push_str("  none with model-specific live evidence\n");
    } else {
        for provider in &summary.providers {
            out.push_str(&format!(
                "  - {}: {}/{} ({:.2}%)\n",
                provider.provider_id,
                provider.covered_model_pairs,
                provider.total_model_pairs,
                provider.coverage_percent
            ));
        }
    }

    if !summary
        .known_provider_ids_without_live_model_coverage
        .is_empty()
    {
        out.push_str("\nKnown providers with no model-specific live evidence:\n");
        for provider_id in &summary.known_provider_ids_without_live_model_coverage {
            out.push_str(&format!("  - {provider_id}\n"));
        }
    }

    if !summary.uncovered_pairs.is_empty() {
        let shown = summary.uncovered_pairs.len().min(gap_limit);
        out.push_str(&format!(
            "\nUncovered provider/model gaps (showing {shown} of {}):\n",
            summary.uncovered_pairs.len()
        ));
        for pair in summary.uncovered_pairs.iter().take(gap_limit) {
            let mut gaps = pair.missing_checkpoints.clone();
            gaps.extend(
                pair.non_passing_checkpoints
                    .iter()
                    .map(|(checkpoint, status)| format!("{checkpoint}:{status:?}")),
            );
            out.push_str(&format!(
                "  - {} / {}: {}\n",
                pair.provider_id,
                pair.model,
                gaps.join(", ")
            ));
        }
    }

    if !summary.issue_driven_targets.is_empty() {
        out.push_str("\nIssue-driven live provider targets:\n");
        for target in &summary.issue_driven_targets {
            let model = target.model.as_deref().unwrap_or("any live model");
            let issues = target.issue_refs.join(", ");
            let observed = if target.observed_models.is_empty() {
                "none".to_string()
            } else {
                target.observed_models.join(", ")
            };
            out.push_str(&format!(
                "  - {} / {} [{}]: {} (observed: {})\n",
                target.provider_id, model, issues, target.status, observed
            ));
            if let Some((model, missing)) = target.missing_checkpoints_by_model.iter().next() {
                let shown_missing = missing.iter().take(6).cloned().collect::<Vec<_>>();
                let suffix = if missing.len() > shown_missing.len() {
                    format!(", +{} more", missing.len() - shown_missing.len())
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "    missing for {model}: {}{}\n",
                    shown_missing.join(", "),
                    suffix
                ));
            }
        }
    }

    out
}

pub fn fingerprint_secret(secret: &str) -> Option<String> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    Some(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn event_id(recorded_at: &DateTime<Utc>, test_name: &str, provider_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(recorded_at.to_rfc3339().as_bytes());
    hasher.update(b"\0");
    hasher.update(test_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(provider_id.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("live_{}", &digest[..16])
}

fn sanitize_json_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_secret_like_text(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_json_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, sanitize_json_value(value)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_secret_like_text(text: &str) -> String {
    let trimmed = text.trim();
    if looks_secret_like(trimmed) {
        "[REDACTED_SECRET]".to_string()
    } else {
        text.to_string()
    }
}

fn looks_secret_like(text: &str) -> bool {
    if text.len() < 16 {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("oc_")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("ya29.")
        || lower.contains("api_key=")
        || lower.contains("authorization: bearer")
        || lower.contains("bearer ")
}

pub fn concise_model_sample(models: &[String], limit: usize) -> Value {
    let sample = models.iter().take(limit).cloned().collect::<Vec<String>>();
    json!({
        "count": models.len(),
        "sample": sample,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value.as_os_str());
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => crate::env::set_var(self.key, value),
                None => crate::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn live_verification_ledger_writes_events_and_coverage_without_secret() {
        let temp = TempDir::new().expect("temp dir");
        let events_path = temp.path().join("events.jsonl");
        let coverage_path = temp.path().join("coverage.json");
        let _events = EnvGuard::set(LEDGER_ENV, &events_path);
        let _coverage = EnvGuard::set(COVERAGE_ENV, &coverage_path);
        let secret = "sk-live-secret-that-must-not-appear";

        let event = LiveVerificationEvent::new(
            "live_test",
            "opencode",
            "OpenCode Zen",
            LiveVerificationAuth::from_secret("test env", Some("OPENCODE_API_KEY"), secret),
            LiveVerificationResult::Passed,
        )
        .with_model("kimi-k2.6")
        .with_capabilities([
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            checkpoints::TOOL_CALL_PARSE,
        ])
        .with_expected_checkpoints([
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            checkpoints::TOOL_CALL_PARSE,
            checkpoints::STREAMING_CHAT_COMPLETION,
        ])
        .with_stage(
            LiveVerificationStage::passed(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT)
                .with_evidence("authorization", Value::String(format!("Bearer {secret}"))),
        )
        .with_not_run_for_missing_expected_checkpoints("not part of this unit test");

        let paths = append_event(&event).expect("append event");
        assert_eq!(paths.events, events_path);
        assert_eq!(paths.coverage, coverage_path);

        let raw_events = std::fs::read_to_string(&paths.events).expect("events raw");
        assert!(!raw_events.contains(secret));
        assert!(raw_events.contains("[REDACTED_SECRET]"));
        assert!(raw_events.contains("sha256:"));

        let raw_coverage = std::fs::read_to_string(&paths.coverage).expect("coverage raw");
        assert!(!raw_coverage.contains(secret));
        assert!(
            raw_coverage
                .contains("opencode::kimi-k2.6::model_catalog_live_endpoint+tool_call_parse")
        );
        assert!(raw_coverage.contains("readiness_gaps"));
        assert!(raw_coverage.contains("streaming_chat_completion"));
        assert!(raw_coverage.contains("checkpoint_taxonomy"));
        assert!(raw_coverage.contains("Auth UX key entry"));
    }

    #[test]
    fn end_to_end_checkpoint_taxonomy_covers_user_ready_ladder() {
        let ids = end_to_end_checkpoint_ids().collect::<Vec<_>>();
        let unique = ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), unique.len(), "checkpoint ids must be unique");
        for required in [
            checkpoints::AUTH_UX_KEY_ENTRY,
            checkpoints::AUTH_CREDENTIAL_LOADED,
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
            checkpoints::PICKER_LIVE_MODELS,
            checkpoints::PICKER_FALLBACK_LABELING,
            checkpoints::MODEL_SWITCH_ROUTE,
            checkpoints::STREAMING_CHAT_COMPLETION,
            checkpoints::TOOL_CALL_PARSE,
            checkpoints::TOOL_EXECUTION_LOOP,
            checkpoints::TOOL_RESULT_FOLLOWUP,
            checkpoints::REAL_JCODE_TOOL_SMOKE,
            checkpoints::RESTART_PERSISTENCE,
            checkpoints::NEGATIVE_ERROR_UX,
            checkpoints::MODEL_CAPABILITY_MATRIX,
            checkpoints::COST_QUOTA_SAFETY,
        ] {
            assert!(unique.contains(required), "missing checkpoint {required}");
        }
        assert!(
            end_to_end_checkpoint_definitions()
                .iter()
                .any(|checkpoint| checkpoint.spends_balance),
            "taxonomy should identify balance-spending checkpoints"
        );
    }

    #[test]
    fn readiness_gaps_track_expected_checkpoints_not_yet_exercised() {
        let event = LiveVerificationEvent::new(
            "partial_live_test",
            "opencode",
            "OpenCode Zen",
            LiveVerificationAuth::non_secret("test", Some("OPENCODE_API_KEY")),
            LiveVerificationResult::Passed,
        )
        .with_expected_checkpoints([
            checkpoints::AUTH_CREDENTIAL_LOADED,
            checkpoints::STREAMING_CHAT_COMPLETION,
        ])
        .with_stage(LiveVerificationStage::passed(
            checkpoints::AUTH_CREDENTIAL_LOADED,
        ));

        assert_eq!(
            event.readiness_gaps(),
            vec![checkpoints::STREAMING_CHAT_COMPLETION.to_string()]
        );
        assert!(!event.user_ready());

        let complete = event.clone().with_stage(LiveVerificationStage::passed(
            checkpoints::STREAMING_CHAT_COMPLETION,
        ));
        assert!(complete.readiness_gaps().is_empty());
        assert!(complete.user_ready());
    }

    #[test]
    fn auth_fingerprint_is_stable_and_non_reversible() {
        let a = fingerprint_secret("secret-value");
        let b = fingerprint_secret("secret-value");
        let c = fingerprint_secret("different-secret");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(!a.unwrap().contains("secret-value"));
        assert_eq!(fingerprint_secret("   "), None);
    }

    fn coverage_entry(
        provider_id: &str,
        provider_label: &str,
        model: Option<&str>,
        checkpoint_statuses: BTreeMap<String, LiveVerificationStageStatus>,
    ) -> LiveVerificationCoverageEntry {
        LiveVerificationCoverageEntry {
            event_id: format!("event-{provider_id}-{}", model.unwrap_or("star")),
            recorded_at: Utc::now(),
            test_name: "coverage-unit".to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_label.to_string(),
            model: model.map(ToString::to_string),
            capabilities: vec!["unit".to_string()],
            expected_checkpoints: checkpoint_statuses.keys().cloned().collect(),
            result: LiveVerificationResult::Passed,
            retest_after: Utc::now() + Duration::days(1),
            jcode_version: "test".to_string(),
            jcode_git_hash: "test".to_string(),
            jcode_git_dirty: false,
            user_ready: false,
            readiness_gaps: Vec::new(),
            checkpoint_statuses,
            stage_statuses: Vec::new(),
        }
    }

    fn strict_statuses(
        overrides: &[(&'static str, LiveVerificationStageStatus)],
    ) -> BTreeMap<String, LiveVerificationStageStatus> {
        let mut statuses = STRICT_PROVIDER_MODEL_COVERAGE_CHECKPOINTS
            .iter()
            .map(|checkpoint| {
                (
                    (*checkpoint).to_string(),
                    LiveVerificationStageStatus::Passed,
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (checkpoint, status) in overrides {
            statuses.insert((*checkpoint).to_string(), status.clone());
        }
        statuses
    }

    #[test]
    fn strict_live_provider_model_coverage_counts_only_exact_pairs_with_all_required_passed() {
        let mut latest = BTreeMap::new();
        let mut missing_tool_result = strict_statuses(&[]);
        missing_tool_result.remove(checkpoints::TOOL_RESULT_FOLLOWUP);
        latest.insert(
            "opencode-zen::model-a::partial".to_string(),
            coverage_entry("opencode", "OpenCode", Some("model-a"), missing_tool_result),
        );
        latest.insert(
            "opencode::model-a::tool-followup".to_string(),
            coverage_entry(
                "opencode-zen",
                "OpenCode Zen",
                Some("model-a"),
                BTreeMap::from([(
                    checkpoints::TOOL_RESULT_FOLLOWUP.to_string(),
                    LiveVerificationStageStatus::Passed,
                )]),
            ),
        );
        latest.insert(
            "opencode::model-b::failed-stream".to_string(),
            coverage_entry(
                "opencode",
                "OpenCode",
                Some("model-b"),
                strict_statuses(&[(
                    checkpoints::STREAMING_CHAT_COMPLETION,
                    LiveVerificationStageStatus::Failed,
                )]),
            ),
        );
        latest.insert(
            "opencode::*::catalog".to_string(),
            coverage_entry("opencode", "OpenCode", None, strict_statuses(&[])),
        );
        let coverage = LiveVerificationCoverage {
            schema_version: SCHEMA_VERSION,
            updated_at: Utc::now(),
            checkpoint_taxonomy_version: CHECKPOINT_TAXONOMY_VERSION,
            checkpoint_taxonomy: checkpoint_catalog_metadata(),
            latest,
        };

        let summary = strict_live_provider_model_coverage_summary(&coverage, "unit");
        assert_eq!(summary.total_provider_model_pairs, 2);
        assert_eq!(summary.covered_provider_model_pairs, 1);
        assert_eq!(summary.coverage_percent, 50.0);
        assert_eq!(summary.covered_pairs[0].provider_id, "opencode");
        assert_eq!(summary.covered_pairs[0].model, "model-a");
        assert_eq!(
            summary.covered_pairs[0].source_provider_ids,
            vec!["opencode".to_string(), "opencode-zen".to_string()]
        );
        assert_eq!(summary.uncovered_pairs[0].model, "model-b");
        assert_eq!(
            summary.uncovered_pairs[0]
                .non_passing_checkpoints
                .get(checkpoints::STREAMING_CHAT_COMPLETION),
            Some(&LiveVerificationStageStatus::Failed)
        );
        assert_eq!(summary.provider_only_entries, vec!["opencode::*::catalog"]);
        assert!(
            !summary
                .known_provider_ids_without_live_model_coverage
                .contains(&"opencode".to_string()),
            "observed providers should not be reported as having no live model evidence"
        );
    }

    #[test]
    fn strict_live_provider_model_coverage_report_names_required_e2e_checkpoints() {
        let coverage = LiveVerificationCoverage {
            schema_version: SCHEMA_VERSION,
            updated_at: Utc::now(),
            checkpoint_taxonomy_version: CHECKPOINT_TAXONOMY_VERSION,
            checkpoint_taxonomy: checkpoint_catalog_metadata(),
            latest: BTreeMap::new(),
        };

        let summary = strict_live_provider_model_coverage_summary(&coverage, "unit");
        let report = format_strict_live_provider_model_coverage_summary(&summary, 10);
        assert!(report.contains("Coverage: 0/0 provider/model pairs"));
        for checkpoint in [
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            checkpoints::PICKER_LIVE_MODELS,
            checkpoints::MODEL_SWITCH_ROUTE,
            checkpoints::STREAMING_CHAT_COMPLETION,
            checkpoints::TOOL_CALL_PARSE,
            checkpoints::TOOL_EXECUTION_LOOP,
            checkpoints::TOOL_RESULT_FOLLOWUP,
            checkpoints::REAL_JCODE_TOOL_SMOKE,
        ] {
            assert!(report.contains(checkpoint), "report missing {checkpoint}");
        }
    }

    #[test]
    fn issue_driven_live_provider_targets_report_covered_partial_and_missing_evidence() {
        let mut latest = BTreeMap::new();
        latest.insert(
            "xiaomi-mimo::mimo-v2.5::strict".to_string(),
            coverage_entry(
                "xiaomi-mimo",
                "Xiaomi MiMo",
                Some("mimo-v2.5"),
                strict_statuses(&[]),
            ),
        );
        latest.insert(
            "gemini::gemini-2.5-pro::partial".to_string(),
            coverage_entry(
                "gemini",
                "Google Gemini",
                Some("gemini-2.5-pro"),
                BTreeMap::from([(
                    checkpoints::NON_STREAMING_CHAT_COMPLETION.to_string(),
                    LiveVerificationStageStatus::Passed,
                )]),
            ),
        );
        let coverage = LiveVerificationCoverage {
            schema_version: SCHEMA_VERSION,
            updated_at: Utc::now(),
            checkpoint_taxonomy_version: CHECKPOINT_TAXONOMY_VERSION,
            checkpoint_taxonomy: checkpoint_catalog_metadata(),
            latest,
        };

        let summary = strict_live_provider_model_coverage_summary(&coverage, "unit");
        let xiaomi = summary
            .issue_driven_targets
            .iter()
            .find(|target| target.provider_id == "xiaomi-mimo")
            .expect("xiaomi target should be tracked");
        assert_eq!(xiaomi.status, "strict_covered");
        assert_eq!(xiaomi.covered_models, vec!["mimo-v2.5"]);
        assert_eq!(xiaomi.issue_refs, vec!["#223"]);

        let gemini = summary
            .issue_driven_targets
            .iter()
            .find(|target| target.provider_id == "gemini")
            .expect("gemini target should be tracked");
        assert_eq!(gemini.status, "observed_missing_strict_checkpoints");
        assert_eq!(gemini.observed_models, vec!["gemini-2.5-pro"]);
        assert!(
            gemini
                .missing_checkpoints_by_model
                .get("gemini-2.5-pro")
                .is_some_and(|missing| missing
                    .contains(&checkpoints::MODEL_CATALOG_LIVE_ENDPOINT.to_string()))
        );

        let nvidia = summary
            .issue_driven_targets
            .iter()
            .find(|target| target.provider_id == "nvidia-nim")
            .expect("nvidia target should be tracked");
        assert_eq!(nvidia.status, "no_model_specific_live_evidence");

        let report = format_strict_live_provider_model_coverage_summary(&summary, 10);
        assert!(report.contains("Issue-driven live provider targets"));
        assert!(report.contains("xiaomi-mimo / mimo-v2.5 [#223]: strict_covered"));
    }
}
