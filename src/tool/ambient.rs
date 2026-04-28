use super::{Tool, ToolContext, ToolOutput};
use crate::ambient::{
    AmbientCycleResult, AmbientManager, AmbientState, CycleStatus, Priority, ScheduleRequest,
    ScheduleTarget,
};
use crate::ambient_runner::AmbientRunnerHandle;
use crate::safety::{self, PermissionRequest, PermissionResult, SafetySystem, Urgency};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Global state for ambient tools
// ---------------------------------------------------------------------------

/// Global ambient cycle result, set by EndAmbientCycleTool for the ambient
/// runner to collect after the cycle completes.
static AMBIENT_CYCLE_RESULT: OnceLock<Mutex<Option<AmbientCycleResult>>> = OnceLock::new();

fn cycle_result_slot() -> &'static Mutex<Option<AmbientCycleResult>> {
    AMBIENT_CYCLE_RESULT.get_or_init(|| Mutex::new(None))
}

/// Store a cycle result for the ambient runner to pick up.
pub fn store_cycle_result(result: AmbientCycleResult) {
    if let Ok(mut slot) = cycle_result_slot().lock() {
        *slot = Some(result);
    }
}

/// Take the stored cycle result (returns None if not set or already taken).
pub fn take_cycle_result() -> Option<AmbientCycleResult> {
    cycle_result_slot()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

/// Global SafetySystem instance shared with ambient tools.
static SAFETY_SYSTEM: OnceLock<Arc<SafetySystem>> = OnceLock::new();
/// Shared schedule/ambient runner handle used to wake the background loop after
/// queue changes.
static SCHEDULE_RUNNER: OnceLock<Mutex<Option<AmbientRunnerHandle>>> = OnceLock::new();
/// Session IDs currently allowed to use ambient-only permission workflows.
static AMBIENT_SESSION_IDS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub fn init_safety_system(system: Arc<SafetySystem>) {
    let _ = SAFETY_SYSTEM.set(system);
}

pub fn init_schedule_runner(handle: AmbientRunnerHandle) {
    if let Ok(mut slot) = SCHEDULE_RUNNER.get_or_init(|| Mutex::new(None)).lock() {
        *slot = Some(handle);
    }
}

fn get_safety_system() -> Arc<SafetySystem> {
    SAFETY_SYSTEM
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(SafetySystem::new()))
}

fn ambient_session_ids() -> &'static Mutex<HashSet<String>> {
    AMBIENT_SESSION_IDS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Mark a session ID as ambient-enabled for ambient-only tooling.
pub fn register_ambient_session(session_id: impl Into<String>) {
    if let Ok(mut ids) = ambient_session_ids().lock() {
        ids.insert(session_id.into());
    }
}

/// Remove a session ID from the ambient-enabled set.
pub fn unregister_ambient_session(session_id: &str) {
    if let Ok(mut ids) = ambient_session_ids().lock() {
        ids.remove(session_id);
    }
}

fn is_ambient_session_registered(session_id: &str) -> bool {
    ambient_session_ids()
        .lock()
        .map(|ids| ids.contains(session_id))
        .unwrap_or(false)
}

fn ensure_ambient_session(ctx: &ToolContext) -> Result<()> {
    if is_ambient_session_registered(&ctx.session_id) {
        Ok(())
    } else {
        anyhow::bail!(
            "request_permission is only available to ambient sessions (session '{}')",
            ctx.session_id
        )
    }
}

// ===========================================================================
// EndAmbientCycleTool
// ===========================================================================

pub struct EndAmbientCycleTool;

impl Default for EndAmbientCycleTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EndAmbientCycleTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct EndCycleInput {
    summary: String,
    memories_modified: u32,
    compactions: u32,
    #[serde(default)]
    proactive_work: Option<String>,
    #[serde(default)]
    next_schedule: Option<NextScheduleInput>,
}

#[derive(Deserialize)]
struct NextScheduleInput {
    #[serde(default)]
    wake_in_minutes: Option<u32>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

#[async_trait]
impl Tool for EndAmbientCycleTool {
    fn name(&self) -> &str {
        "end_ambient_cycle"
    }

    fn description(&self) -> &str {
        "End the current ambient cycle."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["summary", "memories_modified", "compactions"],
            "properties": {
                "intent": super::intent_schema_property(),
                "summary": {
                    "type": "string",
                    "description": "Human-readable summary of what was done this cycle"
                },
                "memories_modified": {
                    "type": "integer",
                    "description": "Count of memories created, merged, pruned, or updated"
                },
                "compactions": {
                    "type": "integer",
                    "description": "Number of context compactions during this cycle"
                },
                "proactive_work": {
                    "type": "string",
                    "description": "Description of proactive code changes, if any"
                },
                "next_schedule": {
                    "type": "object",
                    "description": "When to wake next and what to do",
                    "properties": {
                        "wake_in_minutes": {
                            "type": "integer",
                            "description": "Minutes until next wake"
                        },
                        "context": {
                            "type": "string",
                            "description": "What to do next cycle"
                        },
                        "priority": {
                            "type": "string",
                            "enum": ["low", "normal", "high"],
                            "description": "Priority for next cycle"
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: EndCycleInput = serde_json::from_value(input)?;

        let next_schedule = params.next_schedule.map(|ns| ScheduleRequest {
            wake_in_minutes: ns.wake_in_minutes,
            wake_at: None,
            context: ns.context.unwrap_or_default(),
            priority: parse_priority(ns.priority.as_deref()),
            target: ScheduleTarget::Ambient,
            created_by_session: ctx.session_id.clone(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        });

        let now = Utc::now();
        let result = AmbientCycleResult {
            summary: params.summary.clone(),
            memories_modified: params.memories_modified,
            compactions: params.compactions,
            proactive_work: params.proactive_work,
            next_schedule: next_schedule.clone(),
            started_at: now, // approximate; the runner will override if it tracks start time
            ended_at: now,
            status: CycleStatus::Complete,
            conversation: None, // populated by the runner after cycle completes
        };

        // Store for the ambient runner to pick up
        store_cycle_result(result);

        // Also persist state immediately so a crash after this tool but before
        // the runner collects won't lose the cycle.
        if let Ok(mut state) = AmbientState::load() {
            let next_desc = if let Some(ref sched) = next_schedule {
                let mins = sched.wake_in_minutes.unwrap_or(30);
                format!("~{}", crate::ambient::format_minutes_human(mins))
            } else {
                "system default".to_string()
            };

            state.last_run = Some(now);
            state.last_summary = Some(params.summary.clone());
            state.last_compactions = Some(params.compactions);
            state.last_memories_modified = Some(params.memories_modified);
            state.total_cycles += 1;
            let _ = state.save();

            Ok(ToolOutput::new(format!(
                "Ambient cycle ended. Memories modified: {}, compactions: {}. Next wake: {}",
                params.memories_modified, params.compactions, next_desc
            ))
            .with_title("ambient cycle ended".to_string()))
        } else {
            Ok(ToolOutput::new(format!(
                "Ambient cycle ended (state save failed). Summary: {}",
                params.summary
            ))
            .with_title("ambient cycle ended".to_string()))
        }
    }
}

// ===========================================================================
// ScheduleAmbientTool
// ===========================================================================

pub struct ScheduleAmbientTool;

impl Default for ScheduleAmbientTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleAmbientTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ScheduleInput {
    #[serde(default)]
    wake_in_minutes: Option<u32>,
    #[serde(default)]
    wake_at: Option<String>,
    context: String,
    #[serde(default)]
    priority: Option<String>,
}

#[async_trait]
impl Tool for ScheduleAmbientTool {
    fn name(&self) -> &str {
        "schedule_ambient"
    }

    fn description(&self) -> &str {
        "Schedule an ambient task."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["context"],
            "properties": {
                "intent": super::intent_schema_property(),
                "wake_in_minutes": {
                    "type": "integer",
                    "description": "Minutes from now to wake"
                },
                "wake_at": {
                    "type": "string",
                    "description": "ISO 8601 timestamp for when to wake (alternative to wake_in_minutes)"
                },
                "context": {
                    "type": "string",
                    "description": "What to do when waking — stored in the scheduled queue"
                },
                "priority": {
                    "type": "string",
                    "enum": ["low", "normal", "high"],
                    "description": "Priority for this scheduled task (default: normal)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ScheduleInput = serde_json::from_value(input)?;

        let wake_at = if let Some(ref ts) = params.wake_at {
            Some(
                ts.parse::<chrono::DateTime<Utc>>()
                    .map_err(|e| anyhow::anyhow!("Invalid wake_at timestamp: {}", e))?,
            )
        } else {
            None
        };

        let request = ScheduleRequest {
            wake_in_minutes: params.wake_in_minutes,
            wake_at,
            context: params.context.clone(),
            priority: parse_priority(params.priority.as_deref()),
            target: ScheduleTarget::Ambient,
            created_by_session: ctx.session_id,
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        };

        let mut manager = AmbientManager::new()?;
        let id = manager.schedule(request)?;
        nudge_schedule_runner();

        let when = if let Some(ref ts) = params.wake_at {
            ts.clone()
        } else if let Some(mins) = params.wake_in_minutes {
            format!("in {}", crate::ambient::format_minutes_human(mins))
        } else {
            "in 30m (default)".to_string()
        };

        Ok(
            ToolOutput::new(format!("Scheduled ambient task {} for {}", id, when))
                .with_title(format!("scheduled: {}", params.context)),
        )
    }
}

// ===========================================================================
// RequestPermissionTool
// ===========================================================================

pub struct RequestPermissionTool;

impl Default for RequestPermissionTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestPermissionTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct RequestPermissionInput {
    action: String,
    description: String,
    rationale: String,
    #[serde(default)]
    urgency: Option<String>,
    #[serde(default = "default_false")]
    wait: bool,
    #[serde(default)]
    context: Option<Value>,
}

fn default_false() -> bool {
    false
}

fn extract_context_string(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        map.get(*key).and_then(|value| {
            value.as_str().and_then(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
        })
    })
}

fn extract_context_list(map: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(value) = map.get(*key) else {
            continue;
        };

        if let Some(items) = value.as_array() {
            let list: Vec<String> = items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            if !list.is_empty() {
                return list;
            }
        } else if let Some(single) = value.as_str() {
            let trimmed = single.trim();
            if !trimmed.is_empty() {
                return vec![trimmed.to_string()];
            }
        }
    }
    Vec::new()
}

fn build_permission_review_context(
    action: &str,
    description: &str,
    rationale: &str,
    context: Option<&Value>,
) -> Value {
    let context_obj = context.and_then(Value::as_object);

    let summary = context_obj
        .and_then(|m| extract_context_string(m, &["summary", "what", "activity_summary"]))
        .unwrap_or_else(|| description.to_string());

    let why_permission_needed = context_obj
        .and_then(|m| {
            extract_context_string(
                m,
                &[
                    "why_permission_needed",
                    "why",
                    "reason",
                    "rationale",
                    "justification",
                ],
            )
        })
        .unwrap_or_else(|| rationale.to_string());

    let mut review = Map::new();
    review.insert("summary".to_string(), Value::String(summary));
    review.insert(
        "why_permission_needed".to_string(),
        Value::String(why_permission_needed),
    );
    review.insert(
        "requested_action".to_string(),
        Value::String(action.to_string()),
    );

    let string_fields: [(&str, &[&str]); 4] = [
        (
            "current_activity",
            &["current_activity", "activity", "task", "current_task"],
        ),
        (
            "expected_outcome",
            &["expected_outcome", "outcome", "success_criteria", "success"],
        ),
        ("impact", &["impact", "user_impact"]),
        ("rollback_plan", &["rollback_plan", "rollback"]),
    ];

    if let Some(map) = context_obj {
        for (field_name, keys) in string_fields {
            if let Some(value) = extract_context_string(map, keys) {
                review.insert(field_name.to_string(), Value::String(value));
            }
        }

        let list_fields: [(&str, &[&str]); 4] = [
            (
                "planned_steps",
                &["planned_steps", "steps", "plan", "checklist"],
            ),
            ("files", &["files", "file_paths", "planned_files"]),
            ("commands", &["commands", "planned_commands"]),
            ("risks", &["risks", "risk", "safety_risks"]),
        ];

        for (field_name, keys) in list_fields {
            let items = extract_context_list(map, keys);
            if !items.is_empty() {
                review.insert(
                    field_name.to_string(),
                    Value::Array(items.into_iter().map(Value::String).collect()),
                );
            }
        }
    }

    if let Some(raw) = context
        && !raw.is_object()
    {
        review.insert("notes".to_string(), raw.clone());
    }

    Value::Object(review)
}

#[async_trait]
impl Tool for RequestPermissionTool {
    fn name(&self) -> &str {
        "request_permission"
    }

    fn description(&self) -> &str {
        "Request user permission."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action", "description", "rationale"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "description": "The action requiring permission (e.g., 'create_pull_request', 'push', 'edit')"
                },
                "description": {
                    "type": "string",
                    "description": "What the action will do"
                },
                "rationale": {
                    "type": "string",
                    "description": "Why this action is beneficial"
                },
                "urgency": {
                    "type": "string",
                    "enum": ["low", "normal", "high"],
                    "description": "How urgent the permission request is (default: normal)"
                },
                "wait": {
                    "type": "boolean",
                    "description": "If true, block until user decides (with timeout). If false, queue and continue."
                },
                "context": {
                    "type": "object",
                    "description": "Structured reviewer context. Include summary of current work and why permission is needed.",
                    "properties": {
                        "summary": {
                            "type": "string",
                            "description": "One-paragraph summary of what you are currently doing"
                        },
                        "why_permission_needed": {
                            "type": "string",
                            "description": "Why this action needs user approval right now"
                        },
                        "current_activity": {
                            "type": "string",
                            "description": "Current task or ambient objective"
                        },
                        "planned_steps": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Short ordered plan of intended steps"
                        },
                        "files": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Files expected to be created/modified"
                        },
                        "commands": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Commands expected to be executed"
                        },
                        "risks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Known risks or side effects"
                        },
                        "rollback_plan": {
                            "type": "string",
                            "description": "How to back out changes if needed"
                        },
                        "expected_outcome": {
                            "type": "string",
                            "description": "What successful completion should look like"
                        }
                    },
                    "additionalProperties": true
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        ensure_ambient_session(&ctx)?;

        let params: RequestPermissionInput = serde_json::from_value(input)?;

        let urgency = match params.urgency.as_deref() {
            Some("low") => Urgency::Low,
            Some("high") => Urgency::High,
            _ => Urgency::Normal,
        };

        let request_id = safety::new_request_id();
        let now = Utc::now();
        let review = build_permission_review_context(
            &params.action,
            &params.description,
            &params.rationale,
            params.context.as_ref(),
        );
        let mut request_context = json!({
            "session_id": ctx.session_id,
            "message_id": ctx.message_id,
            "tool_call_id": ctx.tool_call_id,
            "working_dir": ctx.working_dir.as_ref().map(|p| p.display().to_string()),
            "requested_at": now.to_rfc3339(),
        });
        if let Some(obj) = request_context.as_object_mut() {
            obj.insert("review".to_string(), review);
            if let Some(user_context) = params.context {
                obj.insert("details".to_string(), user_context);
            }
        }

        let request = PermissionRequest {
            id: request_id.clone(),
            action: params.action.clone(),
            description: params.description.clone(),
            rationale: params.rationale.clone(),
            urgency,
            wait: params.wait,
            created_at: now,
            context: Some(request_context),
        };

        let system = get_safety_system();
        let result = system.request_permission(request);

        let output = match result {
            PermissionResult::Approved { ref message } => {
                let msg = message.as_deref().unwrap_or("no message");
                format!("Permission approved: {}", msg)
            }
            PermissionResult::Denied { ref reason } => {
                let reason = reason.as_deref().unwrap_or("no reason given");
                format!("Permission denied: {}", reason)
            }
            PermissionResult::Queued { ref request_id } => {
                format!(
                    "Permission request queued (id: {}). \
                     Action '{}' is pending user review.",
                    request_id, params.action
                )
            }
            PermissionResult::Timeout => {
                "Permission request timed out. The user did not respond in time.".to_string()
            }
        };

        Ok(ToolOutput::new(output).with_title(format!("permission: {}", params.action)))
    }
}

// ===========================================================================
// ScheduleTool — available to normal sessions to queue future ambient tasks
// ===========================================================================

pub struct ScheduleTool;

impl Default for ScheduleTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ScheduleToolInput {
    task: String,
    #[serde(default)]
    wake_in_minutes: Option<u32>,
    #[serde(default)]
    wake_at: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    relevant_files: Vec<String>,
    #[serde(default)]
    background_context: Option<String>,
    #[serde(default)]
    success_criteria: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "Schedule a task."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["task"],
            "properties": {
                "intent": super::intent_schema_property(),
                "task": {
                    "type": "string",
                    "description": "Task."
                },
                "wake_in_minutes": { "type": "integer" },
                "wake_at": { "type": "string" },
                "priority": {
                    "type": "string",
                    "enum": ["low", "normal", "high"]
                },
                "relevant_files": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "background_context": {
                    "type": "string",
                    "description": "Optional background context for the scheduled task."
                },
                "success_criteria": { "type": "string" },
                "target": {
                    "type": "string",
                    "enum": ["resume", "spawn", "ambient"],
                    "description": "Delivery target. Defaults to resuming the originating session. Use 'spawn' to run in one new child session, or 'ambient' only for shared ambient work."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ScheduleToolInput = serde_json::from_value(input)?;

        if params.wake_in_minutes.is_none() && params.wake_at.is_none() {
            anyhow::bail!(
                "Either wake_in_minutes or wake_at is required. \
                 This tool is for scheduling future tasks."
            );
        }

        let wake_at = if let Some(ref ts) = params.wake_at {
            Some(
                ts.parse::<chrono::DateTime<Utc>>()
                    .map_err(|e| anyhow::anyhow!("Invalid wake_at timestamp: {}", e))?,
            )
        } else {
            None
        };

        let working_dir = ctx.working_dir.as_ref().map(|p| p.display().to_string());

        let git_branch = ctx
            .working_dir
            .as_ref()
            .and_then(|wd| {
                let mut cmd = std::process::Command::new("git");
                cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
                    .current_dir(wd);
                crate::platform::suppress_child_console(&mut cmd);
                cmd.output().ok()
            })
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            });

        let target = parse_schedule_target(params.target.as_deref(), &ctx.session_id)?;
        let target_summary = match &target {
            ScheduleTarget::Ambient => "ambient agent".to_string(),
            ScheduleTarget::Session { session_id } => {
                format!("resume session {}", session_id)
            }
            ScheduleTarget::Spawn { parent_session_id } => {
                format!("spawn one child session from {}", parent_session_id)
            }
        };

        let request = ScheduleRequest {
            wake_in_minutes: params.wake_in_minutes,
            wake_at,
            context: params.task.clone(),
            priority: parse_priority(params.priority.as_deref()),
            target,
            created_by_session: ctx.session_id.clone(),
            working_dir: working_dir.clone(),
            task_description: Some(params.task.clone()),
            relevant_files: params.relevant_files.clone(),
            git_branch,
            additional_context: {
                let mut parts = Vec::new();
                if let Some(ref bg) = params.background_context {
                    parts.push(format!("Background: {}", bg));
                }
                if let Some(ref sc) = params.success_criteria {
                    parts.push(format!("Success criteria: {}", sc));
                }
                parts.push(format!("Scheduled by session: {}", ctx.session_id));
                Some(parts.join("\n"))
            },
        };

        let mut manager = AmbientManager::new()?;
        let id = manager.schedule(request)?;

        let when = if let Some(ref ts) = params.wake_at {
            ts.clone()
        } else if let Some(mins) = params.wake_in_minutes {
            format!("in {}", crate::ambient::format_minutes_human(mins))
        } else {
            "unspecified".to_string()
        };

        let mut summary = format!("Scheduled task '{}' for {} (id: {})", params.task, when, id);
        if let Some(ref wd) = working_dir {
            summary.push_str(&format!("\nWorking directory: {}", wd));
        }
        if !params.relevant_files.is_empty() {
            summary.push_str(&format!(
                "\nRelevant files: {}",
                params.relevant_files.join(", ")
            ));
        }
        summary.push_str(&format!("\nTarget: {}", target_summary));

        Ok(ToolOutput::new(summary).with_title(format!("scheduled: {}", params.task)))
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn parse_priority(s: Option<&str>) -> Priority {
    match s {
        Some("low") => Priority::Low,
        Some("high") => Priority::High,
        _ => Priority::Normal,
    }
}

fn parse_schedule_target(s: Option<&str>, session_id: &str) -> Result<ScheduleTarget> {
    Ok(match s {
        Some("ambient") => ScheduleTarget::Ambient,
        Some("spawn") => ScheduleTarget::Spawn {
            parent_session_id: session_id.to_string(),
        },
        Some("resume") | None => ScheduleTarget::Session {
            session_id: session_id.to_string(),
        },
        Some(other) => anyhow::bail!(
            "Invalid target '{}'. Expected one of: resume, spawn, ambient",
            other
        ),
    })
}

fn nudge_schedule_runner() {
    let runner = SCHEDULE_RUNNER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    if let Some(runner) = runner {
        runner.nudge();
    }
}

// ---------------------------------------------------------------------------
// SendChannelMessageTool — send messages via any configured channel
// ---------------------------------------------------------------------------

pub struct SendChannelMessageTool;

impl Default for SendChannelMessageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SendChannelMessageTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SendChannelMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a user message."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "message": {
                    "type": "string",
                    "description": "The message text to send"
                },
                "channel": {
                    "type": "string",
                    "description": "Optional: specific channel to send to (e.g. 'telegram', 'discord'). Omit to send to all."
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: Value, _context: ToolContext) -> Result<ToolOutput> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: message"))?;

        let channel_name = args.get("channel").and_then(|v| v.as_str());

        let config = crate::config::config();
        let registry = crate::channel::ChannelRegistry::from_config(&config.safety);

        if let Some(name) = channel_name {
            match registry.find_by_name(name) {
                Some(ch) => match ch.send(message).await {
                    Ok(()) => Ok(ToolOutput::new(format!("Message sent via {}.", name))),
                    Err(e) => Ok(ToolOutput::new(format!(
                        "Failed to send via {}: {}",
                        name, e
                    ))),
                },
                None => {
                    let available = registry.channel_names();
                    Ok(ToolOutput::new(format!(
                        "Channel '{}' not found. Available: {}",
                        name,
                        if available.is_empty() {
                            "none configured".to_string()
                        } else {
                            available.join(", ")
                        }
                    )))
                }
            }
        } else {
            let channels = registry.send_enabled();
            if channels.is_empty() {
                return Ok(ToolOutput::new(
                    "No messaging channels configured. Enable telegram or discord in config.",
                ));
            }
            let mut results = Vec::new();
            for ch in &channels {
                match ch.send(message).await {
                    Ok(()) => results.push(format!("✓ {}", ch.name())),
                    Err(e) => results.push(format!("✗ {}: {}", ch.name(), e)),
                }
            }
            Ok(ToolOutput::new(format!(
                "Message sent: {}",
                results.join(", ")
            )))
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[path = "ambient/tests.rs"]
mod tests;
