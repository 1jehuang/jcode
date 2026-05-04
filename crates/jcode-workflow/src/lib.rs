use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const WORKFLOW_STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMode {
    Ralplan,
    Ultrawork,
    Ralph,
}

impl WorkflowMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ralplan => "ralplan",
            Self::Ultrawork => "ultrawork",
            Self::Ralph => "ralph",
        }
    }

    pub fn is_enabled_in_mvp(self) -> bool {
        matches!(self, Self::Ralplan | Self::Ultrawork)
    }

    fn default_max_iterations(self) -> u32 {
        match self {
            Self::Ralplan => 3,
            Self::Ultrawork => 1,
            Self::Ralph => 50,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Planning,
    Drafting,
    ArchitectReview,
    CriticReview,
    Executing,
    Verifying,
    Blocked,
    Completed,
    Cancelled,
    Failed,
}

impl WorkflowPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowActivationSource {
    SlashCommand,
    ExplicitKeyword,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowActivation {
    pub mode: WorkflowMode,
    pub source: WorkflowActivationSource,
    pub raw_trigger: String,
    pub task_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowState {
    pub schema_version: u32,
    pub mode: WorkflowMode,
    pub active: bool,
    pub phase: WorkflowPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    pub iteration: u32,
    pub max_iterations: u32,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl WorkflowState {
    pub fn new(mode: WorkflowMode, phase: WorkflowPhase, now: DateTime<Utc>) -> Self {
        Self {
            schema_version: WORKFLOW_STATE_SCHEMA_VERSION,
            mode,
            active: true,
            phase,
            session_id: None,
            goal_id: None,
            plan_id: None,
            iteration: 1,
            max_iterations: mode.default_max_iterations(),
            started_at: now,
            updated_at: now,
            completed_at: None,
            last_error: None,
        }
    }

    pub fn for_activation(activation: &WorkflowActivation, now: DateTime<Utc>) -> Self {
        Self::new(
            activation.mode,
            initial_phase_for_mode(activation.mode),
            now,
        )
    }

    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal() || !self.active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowTransition {
    pub from_mode: Option<WorkflowMode>,
    pub from_phase: Option<WorkflowPhase>,
    pub to_mode: WorkflowMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowTransitionDecision {
    Allowed,
    Rejected { reason: String },
}

impl WorkflowTransitionDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

pub fn parse_workflow_activation(input: &str) -> Option<WorkflowActivation> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let raw_trigger = parts.next()?;
    let task_text = parts.next().unwrap_or_default().trim().to_string();
    let normalized = raw_trigger.to_ascii_lowercase();

    let mode = match normalized.as_str() {
        "/ralplan" => WorkflowMode::Ralplan,
        "/ulw" | "/ultrawork" => WorkflowMode::Ultrawork,
        _ => return None,
    };

    if !mode.is_enabled_in_mvp() {
        return None;
    }

    Some(WorkflowActivation {
        mode,
        source: WorkflowActivationSource::SlashCommand,
        raw_trigger: normalized,
        task_text,
    })
}

pub fn validate_workflow_transition(
    current: Option<&WorkflowState>,
    requested: WorkflowMode,
) -> WorkflowTransitionDecision {
    if !requested.is_enabled_in_mvp() {
        return WorkflowTransitionDecision::Rejected {
            reason: format!(
                "workflow mode '{}' is not enabled in the MVP",
                requested.as_str()
            ),
        };
    }

    let Some(current) = current else {
        return WorkflowTransitionDecision::Allowed;
    };

    if current.is_terminal() {
        return WorkflowTransitionDecision::Allowed;
    }

    if current.mode == requested {
        return WorkflowTransitionDecision::Rejected {
            reason: format!("workflow mode '{}' is already active", requested.as_str()),
        };
    }

    match (current.mode, current.phase, requested) {
        (WorkflowMode::Ralplan, WorkflowPhase::Completed, WorkflowMode::Ultrawork) => {
            WorkflowTransitionDecision::Allowed
        }
        (WorkflowMode::Ralplan, _, WorkflowMode::Ultrawork) => {
            WorkflowTransitionDecision::Rejected {
                reason: "cannot start ultrawork while ralplan is still active".to_string(),
            }
        }
        (WorkflowMode::Ultrawork, _, WorkflowMode::Ralplan) => {
            WorkflowTransitionDecision::Rejected {
                reason: "cannot start ralplan while ultrawork is active".to_string(),
            }
        }
        (_, _, _) => WorkflowTransitionDecision::Rejected {
            reason: format!(
                "cannot start '{}' while '{}' is active",
                requested.as_str(),
                current.mode.as_str()
            ),
        },
    }
}

pub fn workflow_plan_summary(items: &[jcode_plan::PlanItem]) -> jcode_plan::PlanGraphSummary {
    jcode_plan::summarize_plan_graph(items)
}

pub fn goal_is_workflow_relevant(goal: &jcode_task_types::Goal) -> bool {
    goal.status.is_resumable() || goal.status == jcode_task_types::GoalStatus::Completed
}

pub fn background_task_is_active(status: &jcode_background_types::BackgroundTaskStatus) -> bool {
    matches!(
        status,
        jcode_background_types::BackgroundTaskStatus::Running
    )
}

fn initial_phase_for_mode(mode: WorkflowMode) -> WorkflowPhase {
    match mode {
        WorkflowMode::Ralplan => WorkflowPhase::Planning,
        WorkflowMode::Ultrawork => WorkflowPhase::Planning,
        WorkflowMode::Ralph => WorkflowPhase::Planning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 4, 18, 0, 0).unwrap()
    }

    fn state(mode: WorkflowMode, phase: WorkflowPhase) -> WorkflowState {
        WorkflowState::new(mode, phase, fixed_time())
    }

    #[test]
    fn workflow_mode_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&WorkflowMode::Ralplan).unwrap(),
            "\"ralplan\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowMode::Ultrawork).unwrap(),
            "\"ultrawork\""
        );
    }

    #[test]
    fn workflow_phase_terminal_detection() {
        assert!(WorkflowPhase::Completed.is_terminal());
        assert!(WorkflowPhase::Cancelled.is_terminal());
        assert!(WorkflowPhase::Failed.is_terminal());
        assert!(!WorkflowPhase::Planning.is_terminal());
        assert!(!WorkflowPhase::Executing.is_terminal());
    }

    #[test]
    fn workflow_state_round_trips() {
        let state =
            WorkflowState::new(WorkflowMode::Ralplan, WorkflowPhase::Planning, fixed_time());
        let encoded = serde_json::to_string(&state).unwrap();
        let decoded: WorkflowState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn parse_ralplan_slash_activation() {
        let activation = parse_workflow_activation("/ralplan create a plan").unwrap();
        assert_eq!(activation.mode, WorkflowMode::Ralplan);
        assert_eq!(activation.source, WorkflowActivationSource::SlashCommand);
        assert_eq!(activation.raw_trigger, "/ralplan");
        assert_eq!(activation.task_text, "create a plan");
    }

    #[test]
    fn parse_ulw_slash_activation() {
        let activation = parse_workflow_activation("/ulw do work").unwrap();
        assert_eq!(activation.mode, WorkflowMode::Ultrawork);
        assert_eq!(activation.raw_trigger, "/ulw");
        assert_eq!(activation.task_text, "do work");
    }

    #[test]
    fn parse_ultrawork_slash_activation() {
        let activation = parse_workflow_activation("/ultrawork do work").unwrap();
        assert_eq!(activation.mode, WorkflowMode::Ultrawork);
        assert_eq!(activation.raw_trigger, "/ultrawork");
    }

    #[test]
    fn parse_activation_allows_leading_whitespace() {
        let activation = parse_workflow_activation("   /ulw do work").unwrap();
        assert_eq!(activation.mode, WorkflowMode::Ultrawork);
        assert_eq!(activation.task_text, "do work");
    }

    #[test]
    fn does_not_parse_bare_ulw() {
        assert_eq!(parse_workflow_activation("ulw do work"), None);
    }

    #[test]
    fn does_not_parse_ralph() {
        assert_eq!(parse_workflow_activation("/ralph do work"), None);
    }

    #[test]
    fn does_not_parse_incidental_phrases() {
        for input in [
            "I don't stop the build before tests",
            "this is a must complete list",
            "please make a consensus plan eventually",
        ] {
            assert_eq!(parse_workflow_activation(input), None, "input: {input}");
        }
    }

    #[test]
    fn does_not_parse_mid_sentence_slash() {
        assert_eq!(parse_workflow_activation("please run /ulw on this"), None);
    }

    #[test]
    fn transition_allows_no_active_to_ralplan() {
        assert!(validate_workflow_transition(None, WorkflowMode::Ralplan).is_allowed());
    }

    #[test]
    fn transition_allows_no_active_to_ultrawork() {
        assert!(validate_workflow_transition(None, WorkflowMode::Ultrawork).is_allowed());
    }

    #[test]
    fn transition_rejects_ralph() {
        assert!(!validate_workflow_transition(None, WorkflowMode::Ralph).is_allowed());
    }

    #[test]
    fn transition_rejects_active_ralplan_to_ultrawork() {
        let current = state(WorkflowMode::Ralplan, WorkflowPhase::ArchitectReview);
        assert!(
            !validate_workflow_transition(Some(&current), WorkflowMode::Ultrawork).is_allowed()
        );
    }

    #[test]
    fn transition_allows_completed_ralplan_to_ultrawork() {
        let mut current = state(WorkflowMode::Ralplan, WorkflowPhase::Completed);
        current.active = false;
        current.completed_at = Some(fixed_time());
        assert!(validate_workflow_transition(Some(&current), WorkflowMode::Ultrawork).is_allowed());
    }

    #[test]
    fn transition_rejects_active_ultrawork_to_ralplan() {
        let current = state(WorkflowMode::Ultrawork, WorkflowPhase::Executing);
        assert!(!validate_workflow_transition(Some(&current), WorkflowMode::Ralplan).is_allowed());
    }

    #[test]
    fn transition_allows_terminal_state_to_new_workflow() {
        let current = state(WorkflowMode::Ultrawork, WorkflowPhase::Failed);
        assert!(validate_workflow_transition(Some(&current), WorkflowMode::Ralplan).is_allowed());
    }

    #[test]
    fn bridge_helpers_use_existing_typed_primitives() {
        assert!(background_task_is_active(
            &jcode_background_types::BackgroundTaskStatus::Running
        ));
        assert!(!background_task_is_active(
            &jcode_background_types::BackgroundTaskStatus::Completed
        ));

        let plan = vec![jcode_plan::PlanItem {
            content: "do thing".to_string(),
            status: "queued".to_string(),
            priority: "high".to_string(),
            id: "item-1".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }];
        assert_eq!(
            workflow_plan_summary(&plan).ready_ids,
            vec!["item-1".to_string()]
        );

        let goal = jcode_task_types::Goal::new(
            "Ship workflow modes",
            jcode_task_types::GoalScope::Project,
        );
        assert!(goal_is_workflow_relevant(&goal));
    }
}
