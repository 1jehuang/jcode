use super::{Tool, ToolContext, ToolOutput};
use crate::ambient::{
    AmbientCycleResult, AmbientManager, AmbientState, CycleStatus, Priority, ScheduleRequest,
};
use crate::safety::{self, PermissionRequest, PermissionResult, SafetySystem, Urgency};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
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
    cycle_result_slot().lock().ok().and_then(|mut slot| slot.take())
}

/// Global SafetySystem instance shared with ambient tools.
static SAFETY_SYSTEM: OnceLock<Arc<SafetySystem>> = OnceLock::new();

pub fn init_safety_system(system: Arc<SafetySystem>) {
    let _ = SAFETY_SYSTEM.set(system);
}

fn get_safety_system() -> Arc<SafetySystem> {
    SAFETY_SYSTEM
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(SafetySystem::new()))
}

// ===========================================================================
// EndAmbientCycleTool
// ===========================================================================

pub struct EndAmbientCycleTool;

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
        "End the current ambient cycle. MUST be called at the end of every ambient cycle. \
         Provide a summary of work done, counts of memories modified and compactions, \
         and optionally schedule the next wake."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["summary", "memories_modified", "compactions"],
            "properties": {
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

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: EndCycleInput = serde_json::from_value(input)?;

        let next_schedule = params.next_schedule.map(|ns| ScheduleRequest {
            wake_in_minutes: ns.wake_in_minutes,
            wake_at: None,
            context: ns.context.unwrap_or_default(),
            priority: parse_priority(ns.priority.as_deref()),
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
        };

        // Store for the ambient runner to pick up
        store_cycle_result(result);

        // Also persist state immediately so a crash after this tool but before
        // the runner collects won't lose the cycle.
        if let Ok(mut state) = AmbientState::load() {
            let next_desc = if let Some(ref sched) = next_schedule {
                let mins = sched.wake_in_minutes.unwrap_or(30);
                format!("~{}m", mins)
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
        "Schedule a future ambient task. Provide either wake_in_minutes or wake_at (ISO timestamp), \
         a context string describing what to do, and a priority level."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["context"],
            "properties": {
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

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
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
        };

        let mut manager = AmbientManager::new()?;
        let id = manager.schedule(request)?;

        let when = if let Some(ref ts) = params.wake_at {
            ts.clone()
        } else if let Some(mins) = params.wake_in_minutes {
            format!("in {} minutes", mins)
        } else {
            "in 30 minutes (default)".to_string()
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
}

fn default_false() -> bool {
    false
}

#[async_trait]
impl Tool for RequestPermissionTool {
    fn name(&self) -> &str {
        "request_permission"
    }

    fn description(&self) -> &str {
        "Request user permission for a Tier 2 action (e.g., code changes, PRs, pushes). \
         The request is queued for user review. If wait=true, the tool blocks until a \
         decision is made (with timeout)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action", "description", "rationale"],
            "properties": {
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
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: RequestPermissionInput = serde_json::from_value(input)?;

        let urgency = match params.urgency.as_deref() {
            Some("low") => Urgency::Low,
            Some("high") => Urgency::High,
            _ => Urgency::Normal,
        };

        let request_id = safety::new_request_id();
        let request = PermissionRequest {
            id: request_id.clone(),
            action: params.action.clone(),
            description: params.description.clone(),
            rationale: params.rationale.clone(),
            urgency,
            wait: params.wait,
            created_at: Utc::now(),
            context: None,
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
// Helpers
// ===========================================================================

fn parse_priority(s: Option<&str>) -> Priority {
    match s {
        Some("low") => Priority::Low,
        Some("high") => Priority::High,
        _ => Priority::Normal,
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_priority() {
        assert_eq!(parse_priority(Some("low")), Priority::Low);
        assert_eq!(parse_priority(Some("normal")), Priority::Normal);
        assert_eq!(parse_priority(Some("high")), Priority::High);
        assert_eq!(parse_priority(None), Priority::Normal);
        assert_eq!(parse_priority(Some("unknown")), Priority::Normal);
    }

    #[test]
    fn test_cycle_result_store_and_take() {
        let result = AmbientCycleResult {
            summary: "test".to_string(),
            memories_modified: 1,
            compactions: 0,
            proactive_work: None,
            next_schedule: None,
            started_at: Utc::now(),
            ended_at: Utc::now(),
            status: CycleStatus::Complete,
        };

        store_cycle_result(result);
        let taken = take_cycle_result();
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().summary, "test");

        // Second take should be None
        assert!(take_cycle_result().is_none());
    }

    #[test]
    fn test_end_cycle_input_deserialization() {
        let input = json!({
            "summary": "Merged 3 duplicates",
            "memories_modified": 5,
            "compactions": 1,
            "proactive_work": "Fixed typo in README",
            "next_schedule": {
                "wake_in_minutes": 20,
                "context": "Verify stale facts",
                "priority": "high"
            }
        });

        let parsed: EndCycleInput = serde_json::from_value(input).unwrap();
        assert_eq!(parsed.summary, "Merged 3 duplicates");
        assert_eq!(parsed.memories_modified, 5);
        assert_eq!(parsed.compactions, 1);
        assert_eq!(parsed.proactive_work.as_deref(), Some("Fixed typo in README"));
        let ns = parsed.next_schedule.unwrap();
        assert_eq!(ns.wake_in_minutes, Some(20));
        assert_eq!(ns.context.as_deref(), Some("Verify stale facts"));
        assert_eq!(ns.priority.as_deref(), Some("high"));
    }

    #[test]
    fn test_end_cycle_input_minimal() {
        let input = json!({
            "summary": "Nothing to do",
            "memories_modified": 0,
            "compactions": 0
        });

        let parsed: EndCycleInput = serde_json::from_value(input).unwrap();
        assert_eq!(parsed.summary, "Nothing to do");
        assert!(parsed.proactive_work.is_none());
        assert!(parsed.next_schedule.is_none());
    }

    #[test]
    fn test_schedule_input_deserialization() {
        let input = json!({
            "wake_in_minutes": 15,
            "context": "Check CI results",
            "priority": "normal"
        });

        let parsed: ScheduleInput = serde_json::from_value(input).unwrap();
        assert_eq!(parsed.wake_in_minutes, Some(15));
        assert!(parsed.wake_at.is_none());
        assert_eq!(parsed.context, "Check CI results");
        assert_eq!(parsed.priority.as_deref(), Some("normal"));
    }

    #[test]
    fn test_permission_input_deserialization() {
        let input = json!({
            "action": "create_pull_request",
            "description": "Create PR for test fixes",
            "rationale": "Found failing tests that need attention",
            "urgency": "high",
            "wait": true
        });

        let parsed: RequestPermissionInput = serde_json::from_value(input).unwrap();
        assert_eq!(parsed.action, "create_pull_request");
        assert_eq!(parsed.description, "Create PR for test fixes");
        assert_eq!(parsed.rationale, "Found failing tests that need attention");
        assert_eq!(parsed.urgency.as_deref(), Some("high"));
        assert!(parsed.wait);
    }

    #[test]
    fn test_permission_input_defaults() {
        let input = json!({
            "action": "edit",
            "description": "Fix typo",
            "rationale": "Obvious error"
        });

        let parsed: RequestPermissionInput = serde_json::from_value(input).unwrap();
        assert!(parsed.urgency.is_none());
        assert!(!parsed.wait);
    }
}
