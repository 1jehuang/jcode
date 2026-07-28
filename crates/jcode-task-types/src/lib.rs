use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalScope {
    Global,
    #[default]
    Project,
}

impl GoalScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "global" => Some(Self::Global),
            "project" => Some(Self::Project),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Draft,
    #[default]
    Active,
    Paused,
    Blocked,
    Completed,
    Archived,
    Abandoned,
}

impl GoalStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "draft" => Some(Self::Draft),
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "blocked" => Some(Self::Blocked),
            "completed" => Some(Self::Completed),
            "archived" => Some(Self::Archived),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Archived => "archived",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn sort_rank(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Blocked => 1,
            Self::Draft => 2,
            Self::Paused => 3,
            Self::Completed => 4,
            Self::Archived => 5,
            Self::Abandoned => 6,
        }
    }

    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Active | Self::Blocked | Self::Draft)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GoalStep {
    pub id: String,
    pub content: String,
    #[serde(default = "default_pending_status")]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GoalMilestone {
    pub id: String,
    pub title: String,
    #[serde(default = "default_pending_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<GoalStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalUpdate {
    pub at: DateTime<Utc>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Goal {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub scope: GoalScope,
    #[serde(default)]
    pub status: GoalStatus,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub why: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub milestones: Vec<GoalMilestone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_milestone_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<GoalUpdate>,
}

impl Goal {
    pub fn new(title: &str, scope: GoalScope) -> Self {
        let now = Utc::now();
        let trimmed = title.trim();
        Self {
            id: sanitize_goal_id(trimmed),
            title: trimmed.to_string(),
            scope,
            status: GoalStatus::Active,
            description: String::new(),
            why: String::new(),
            success_criteria: Vec::new(),
            milestones: Vec::new(),
            next_steps: Vec::new(),
            blockers: Vec::new(),
            current_milestone_id: None,
            progress_percent: None,
            created_at: now,
            updated_at: now,
            updates: Vec::new(),
        }
    }

    pub fn current_milestone(&self) -> Option<&GoalMilestone> {
        let current_id = self.current_milestone_id.as_deref()?;
        self.milestones.iter().find(|m| m.id == current_id)
    }
}

pub fn sanitize_goal_id(id: &str) -> String {
    let slug = slugify(id);
    if slug.is_empty() {
        "goal".to_string()
    } else {
        slug
    }
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn default_pending_status() -> String {
    "pending".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
    /// Optional group label. Todos that share a group are displayed together
    /// under a single header. Use one group per coherent goal; when work is
    /// steered into a new area, start a new group instead of renaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Forward-looking confidence, from 0-100, that this todo can be completed correctly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    /// Confidence, from 0-100, recorded when the todo is marked completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_confidence: Option<u8>,
    /// Every distinct confidence value this todo has carried, oldest first,
    /// ending with the current one. Maintained by the todo tool (not the
    /// model): the first entry is the planning-time confidence, later entries
    /// record how the assessment evolved while the item was worked on. This
    /// preserves the planning signal even after the model overwrites
    /// `confidence` when marking the item done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confidence_history: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<String>,
}

/// Plan-level understanding of what the user actually wants, covering the
/// whole todo list rather than one group.
///
/// Intent is a property of the request, not of an individual group of steps,
/// so it is recorded once per plan: what the user is really after, and how
/// faithfully the plan and its feedback loops represent that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoPlan {
    /// The user's underlying reason and desired outcome for this work, kept
    /// distinct from the agent's steps and validation loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_intention: Option<String>,
    /// How well the agent understands what the user actually wants and how
    /// faithfully this plan represents it, from 0-100. It does not measure
    /// implementation progress. Older payloads called this `alignment_score`.
    #[serde(
        default,
        alias = "alignment_score",
        alias = "user_intention_alignment",
        skip_serializing_if = "Option::is_none"
    )]
    pub understands_user_intent: Option<u8>,
    /// Every distinct `understands_user_intent` value this plan has carried,
    /// oldest first, ending with the current one. Maintained by the todo tool,
    /// not the model: understanding of a request typically starts low and rises
    /// as the agent explores, so the trajectory distinguishes an agent that
    /// resolved the ambiguity by investigating from one that never did.
    /// Model-supplied values are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub understands_user_intent_history: Vec<u8>,
}

/// A plan field changed by a todo-tool update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoPlanField {
    UserIntention,
    #[serde(alias = "alignment_score", alias = "user_intention_alignment")]
    UnderstandsUserIntent,
}

/// Before/after state for the plan-level intent assessment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoPlanChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<TodoPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<TodoPlan>,
    pub fields: Vec<TodoPlanField>,
}

/// A goal-level assessment attached to a todo group (or, for an ungrouped
/// flat list, the whole list as one implicit goal with `group: None`).
///
/// A closed feedback loop is a property of an objective, not of individual
/// steps: "optimize grep latency" can close its loop because progress has a
/// metric, while "design an onboarding screen" cannot because success is a
/// taste judgment. Items like "read the auth code" have no meaningful score of
/// their own, so the score lives here instead of on `TodoItem`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoGoal {
    /// Group label this goal describes. `None` covers the ungrouped list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// From 0-100: how much of this goal's correctness `feedback_loop` can
    /// report on its own, without the agent's judgment or the user's.
    #[serde(
        default,
        alias = "hill_climbability",
        skip_serializing_if = "Option::is_none"
    )]
    pub closed_feedback_loop: Option<u8>,
    /// Every distinct `closed_feedback_loop` value this goal has carried, oldest
    /// first. Tool-maintained; model-supplied values are ignored.
    #[serde(
        default,
        alias = "hill_climbability_history",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub closed_feedback_loop_history: Vec<u8>,
    /// The concrete feedback loop used to judge whether each iteration improves
    /// the outcome (e.g. a benchmark command and the metric it reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_loop: Option<String>,
    /// How completely the agent owned the goal's full outcome, including the
    /// requested work, reasonably necessary adjacent work, end-to-end
    /// validation, cleanup, and explicit disclosure of remaining gaps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_to_end_ownership: Option<u8>,
    /// Every distinct `end_to_end_ownership` value this goal has carried,
    /// oldest first. Tool-maintained; model-supplied values are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub end_to_end_ownership_history: Vec<u8>,
}

/// A goal field changed by a todo-tool update. This lets transcript renderers
/// show a concise quality-gate refinement instead of repeating the full plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoGoalField {
    #[serde(alias = "hill_climbability")]
    ClosedFeedbackLoop,
    FeedbackLoop,
    EndToEndOwnership,
}

/// Before/after state for one changed todo goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoGoalChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<TodoGoal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<TodoGoal>,
    pub fields: Vec<TodoGoalField>,
}

use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedCatchupState {
    #[serde(default)]
    pub seen_at_ms_by_session: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
pub struct CatchupBrief {
    pub reason: String,
    pub tags: Vec<String>,
    pub last_user_prompt: Option<String>,
    pub activity_steps: Vec<String>,
    pub files_touched: Vec<String>,
    pub tool_counts: Vec<(String, usize)>,
    pub validation_notes: Vec<String>,
    pub latest_agent_response: Option<String>,
    pub needs_from_user: String,
    pub updated_at: DateTime<Utc>,
}
