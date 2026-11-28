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

/// Declare a semantic assessment enum that serializes and deserializes as a
/// snake_case string.
macro_rules! semantic_state {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $variant:ident = $label:literal $(| $legacy:literal)*, score: $score:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $( $variant, )+
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $label, )+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value.trim().to_ascii_lowercase().as_str() {
                    $( $label => Some(Self::$variant), )+
                    _ => None,
                }
            }

            /// Representative 0-100 score for telemetry aggregation.
            pub fn telemetry_score(&self) -> u8 {
                match self {
                    $( Self::$variant => $score, )+
                }
            }

            /// How many ordered levels separate two states.
            pub fn level(&self) -> u8 {
                *self as u8
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                use serde::de::Error;
                let raw = String::deserialize(deserializer)?;
                match raw.trim().to_ascii_lowercase().as_str() {
                    $( $( $legacy => return Ok(Self::$variant), )* )+
                    _ => {}
                }
                Self::parse(&raw).ok_or_else(|| {
                    D::Error::custom(format!(
                        concat!("invalid ", stringify!($name), " state: {}"),
                        raw
                    ))
                })
            }
        }
    };
}

semantic_state! {
    /// How well the agent understands what the user actually wants.
    IntentUnderstanding {
        Uncertain = "uncertain", score: 40,
        Partial = "partial", score: 80,
        Clear = "clear", score: 100,
    }
}

semantic_state! {
    /// Whether an iterative goal has a defensible reason to stop. Unlike
    /// feedback-loop quality, this records how far that loop has actually been
    /// exercised rather than whether progress can be measured in principle.
    IterationMaturity {
        NotStarted = "not_started", score: 6,
        Exploring = "exploring", score: 21,
        Improving = "improving", score: 40,
        PlateauUnproven = "plateau_unproven", score: 60,
        OutcomeReached = "outcome_reached", score: 74,
        ConstraintsExhausted = "constraints_exhausted", score: 84,
        PlateauConfirmed = "plateau_confirmed", score: 92,
        BudgetExhausted = "budget_exhausted", score: 98,
    }
}

impl IterationMaturity {
    /// Terminal bases that can justify completing a goal. `BudgetExhausted` is
    /// deliberately terminal but not "better" than a confirmed plateau.
    pub fn permits_completion(self) -> bool {
        matches!(
            self,
            Self::OutcomeReached
                | Self::ConstraintsExhausted
                | Self::PlateauConfirmed
                | Self::BudgetExhausted
        )
    }
}

semantic_state! {
    /// How much of a goal's correctness its feedback loop can report on its
    /// own, without the agent's or the user's judgment.
    FeedbackLoopState {
        Absent = "absent", score: 10,
        Weak = "weak", score: 35,
        Usable = "usable", score: 65,
        Strong = "strong", score: 88,
        Closed = "closed", score: 98,
    }
}

semantic_state! {
    /// How directly a goal's feedback loop represents the behavior or outcome
    /// the user will actually accept.
    FeedbackLoopRelevance {
        Indirect = "indirect", score: 12,
        Synthetic = "synthetic", score: 37,
        Representative = "representative", score: 75,
        AcceptanceBlocked = "acceptance_blocked", score: 88,
        AcceptanceAligned = "acceptance_aligned", score: 98,
    }
}

semantic_state! {
    /// How broadly a goal's feedback loop exercises the paths on which the
    /// result can succeed or fail.
    FeedbackLoopCoverage {
        Narrow = "narrow", score: 25,
        MainPaths = "main_paths", score: 75,
        EdgeAndIntegrationPaths = "edge_and_integration_paths", score: 98,
    }
}

semantic_state! {
    /// How completely a goal's explicit requirements and changed public outputs
    /// are connected to concrete checks and observed results.
    FeedbackLoopTraceability {
        Unmapped = "unmapped", score: 25,
        Partial = "partial", score: 75,
        Complete = "complete", score: 98,
    }
}

semantic_state! {
    /// Evidence state behind a todo: from an unexamined guess to a result
    /// verified end to end.
    ConfidenceState {
        Speculative = "speculative", score: 40,
        Plausible = "plausible", score: 80,
        Validated = "validated", score: 96,
        Verified = "verified", score: 100,
    }
}

impl ConfidenceState {
    pub fn from_level(level: u8) -> Self {
        match level {
            0 => Self::Speculative,
            1 => Self::Plausible,
            2 => Self::Validated,
            _ => Self::Verified,
        }
    }
}

semantic_state! {
    /// Intrinsic difficulty of a goal. Descriptive, never gated: it only
    /// calibrates how much delivery follow-through a completion review expects.
    Difficulty {
        Trivial = "trivial", score: 6,
        Routine = "routine", score: 19,
        Involved = "involved", score: 32,
        Complex = "complex", score: 45,
        Hard = "hard", score: 58,
        Expert = "expert", score: 71,
        Research = "research", score: 84,
        OpenEnded = "open_ended", score: 95,
    }
}

semantic_state! {
    /// How far beyond the literal request the agent's work extended.
    /// Descriptive, never gated.
    Autonomy {
        RequestedOnly = "requested_only", score: 12,
        NecessaryFollowthrough = "necessary_followthrough", score: 38,
        Proactive = "proactive", score: 62,
        Stewardship = "stewardship", score: 88,
    }
}

semantic_state! {
    /// How far a goal's result actually traveled toward the user's outcome.
    DeliveryState {
        Undelivered = "undelivered" | "not_started" | "change_made" | "integrated" | "workflow_validated", score: 0,
        PartiallyDelivered = "partially_delivered", score: 50,
        Delivered = "delivered" | "outcome_delivered", score: 98,
    }
}

semantic_state! {
    /// How the result was checked, independent of whether it reached the user.
    VerificationState {
        Unchecked = "unchecked", score: 0,
        CheckedInIsolation = "checked_in_isolation", score: 50,
        CheckedEndToEnd = "checked_end_to_end", score: 98,
    }
}

/// The todo is paused until the user completes an authentication the agent
/// has no access to (a login, 2FA prompt, or credential). Not for approvals
/// or decisions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoUserAuth {
    #[serde(deserialize_with = "deserialize_nonempty_auth_reason")]
    pub reason: String,
}

fn deserialize_nonempty_auth_reason<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.trim().is_empty() {
        return Err(serde::de::Error::custom(
            "requires_user_auth reason must be nonempty",
        ));
    }
    Ok(value)
}

/// `TodoItem::lane_owner` value that requests an auto-spawned swarm worker.
pub const LANE_OWNER_WORKER: &str = "worker";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
    /// Optional group label. Todos that share a group are displayed together
    /// under a single header. Use one group per coherent goal; when work is
    /// steered into a new area, start a new group instead of renaming.
    /// Also accepted as `lane`: the lane this todo belongs to.
    #[serde(default, alias = "lane", skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Explicit prerequisite todo IDs for this lane. `None` retains the default
    /// phase barrier. All lane members carry the same nonempty list, referring
    /// only to todos in earlier phases by first appearance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_after: Option<Vec<String>>,
    /// Who runs this lane: [`LANE_OWNER_WORKER`] makes the swarm server spawn a
    /// dedicated worker as soon as the lane is ready; anything else (default)
    /// means this todo list's own agent runs it. Lane-wide, like `lane_after`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_owner: Option<String>,
    /// Forward-looking evidence state that this todo can be completed
    /// correctly. Legacy sessions stored a 0-100 score; numbers map onto the
    /// closest state on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ConfidenceState>,
    /// Evidence state recorded when the todo is marked completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_confidence: Option<ConfidenceState>,
    /// Every distinct confidence state this todo has carried, oldest first,
    /// ending with the current one. Maintained by the todo tool (not the
    /// model): the first entry is the planning-time assessment, later entries
    /// record how it evolved while the item was worked on. This preserves the
    /// planning signal even after the model overwrites `confidence` when
    /// marking the item done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confidence_history: Vec<ConfidenceState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// Existing background task or durable swarm await IDs in this session.
    /// Dependencies block only while pending, never after completion or timeout.
    /// An empty array or null clears dependencies.
    #[serde(
        default,
        deserialize_with = "deserialize_waiting_on",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub waiting_on: Vec<String>,
    /// Why this todo waits on `waiting_on`. The tool boundary presents the
    /// pair as one `{ids, reason}` object. Meaningless without dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_reason: Option<String>,
    /// Paused until the user authenticates. Items have no other block: legacy
    /// `external_blocker` and `completely_blocked` fields are ignored on load,
    /// so old sessions resume as ordinary actionable work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_user_auth: Option<TodoUserAuth>,
    /// Worker session ID, or reserved `@self` for an explicit start by this
    /// todo list's agent. Absence means ownership is unknown, not local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<String>,
    /// Optional execution stage. Stages run in order: stage 2 starts only
    /// after every todo in stage 1 is done. Inside a stage, todos that share a
    /// `group` form one lane that runs in list order, and different lanes run
    /// in parallel. `None` means "sequential in list order". Deliberately a
    /// plain number. Optional `lane_after` narrows the default phase barrier
    /// to explicit earlier-phase prerequisites. Older sessions stored this as `wave`.
    #[serde(
        default,
        alias = "wave",
        alias = "phase",
        deserialize_with = "deserialize_optional_stage",
        skip_serializing_if = "Option::is_none"
    )]
    pub stage: Option<u32>,
    /// Optional longer brief: file paths, acceptance criteria, context. Never
    /// drawn on the todo card (which shows only the short `content` title),
    /// but appended to a swarm worker's assignment so short titles do not
    /// starve workers of context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

fn deserialize_waiting_on<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Vec<String>>::deserialize(deserializer)? {
        Some(dependencies) => Ok(dependencies),
        // Null explicitly clears dependency references, just like an empty list.
        None => Ok(Vec::new()),
    }
}

/// Accept `2`, `2.0`, `"2"`, `"stage 2"`, `""`, or `null` for a todo stage.
/// Zero and unparseable values read as absent rather than failing the whole
/// todo write, so a sloppy stage can never cost the model its plan.
fn deserialize_optional_stage<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| parse_stage_value(&value)))
}

pub fn parse_stage_value(value: &serde_json::Value) -> Option<u32> {
    let stage = match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f.round() as u64)),
        serde_json::Value::String(s) => {
            let digits: String = s
                .trim()
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.parse::<u64>().ok()
        }
        _ => None,
    }?;
    u32::try_from(stage).ok().filter(|stage| *stage > 0)
}

/// Plan-level understanding of what the user actually wants, covering the
/// whole todo list rather than one group.
///
/// Intent is a property of the request, not of an individual group of steps,
/// so it is recorded once per plan: what the user is really after, and how
/// faithfully the plan and its feedback loops represent that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoPlan {
    /// Short session name (a few words) chosen by the agent. Used as the
    /// session title in pickers ahead of lane names and generated titles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The user's underlying reason and desired outcome for this work, kept
    /// distinct from the agent's steps and validation loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_intention: Option<String>,
    /// How well the agent understands what the user actually wants and how
    /// faithfully this plan represents it. It does not measure implementation
    /// progress. Older payloads stored a 0-100 score under this name or
    /// `alignment_score`; numbers map onto the closest state on load.
    #[serde(
        default,
        alias = "alignment_score",
        alias = "user_intention_alignment",
        skip_serializing_if = "Option::is_none"
    )]
    pub understands_user_intent: Option<IntentUnderstanding>,
    /// Every distinct `understands_user_intent` state this plan has carried,
    /// oldest first, ending with the current one. Maintained by the todo tool,
    /// not the model: understanding of a request typically starts low and rises
    /// as the agent explores, so the trajectory distinguishes an agent that
    /// resolved the ambiguity by investigating from one that never did.
    /// Model-supplied values are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub understands_user_intent_history: Vec<IntentUnderstanding>,
}

/// A plan field changed by a todo-tool update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoPlanField {
    Title,
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
#[serde(remote = "Self")]
pub struct TodoGoal {
    /// Group label this goal describes. `None` covers the ungrouped list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// How much of this goal's correctness `feedback_loop` can report on its
    /// own, without the agent's judgment or the user's. Legacy sessions stored
    /// a 0-100 score; numbers map onto the closest state on load.
    #[serde(
        default,
        alias = "hill_climbability",
        skip_serializing_if = "Option::is_none"
    )]
    pub closed_feedback_loop: Option<FeedbackLoopState>,
    /// Every distinct `closed_feedback_loop` state this goal has carried, oldest
    /// first. Tool-maintained; model-supplied values are ignored.
    #[serde(
        default,
        alias = "hill_climbability_history",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub closed_feedback_loop_history: Vec<FeedbackLoopState>,
    /// The concrete feedback loop used to judge whether each iteration improves
    /// the outcome (e.g. a benchmark command and the metric it reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_loop: Option<String>,
    /// How directly `feedback_loop` represents the public behavior and outcome
    /// the user will actually judge, rather than a proxy or internal detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_loop_relevance: Option<FeedbackLoopRelevance>,
    /// Every distinct `feedback_loop_relevance` state this goal has carried,
    /// oldest first. Tool-maintained; model-supplied values are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback_loop_relevance_history: Vec<FeedbackLoopRelevance>,
    /// How broadly `feedback_loop` exercises main paths, integration boundaries,
    /// edge cases, packaging, and likely failure modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_loop_coverage: Option<FeedbackLoopCoverage>,
    /// Every distinct `feedback_loop_coverage` state this goal has carried,
    /// oldest first. Tool-maintained; model-supplied values are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback_loop_coverage_history: Vec<FeedbackLoopCoverage>,
    /// Whether every explicit requirement and changed public output is mapped to
    /// a concrete check and its observed result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_loop_traceability: Option<FeedbackLoopTraceability>,
    /// Every distinct `feedback_loop_traceability` state this goal has carried,
    /// oldest first. Tool-maintained; model-supplied values are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback_loop_traceability_history: Vec<FeedbackLoopTraceability>,
    /// How far the goal's result actually traveled toward the user's outcome:
    /// required on incoming goal assessments. Historical goals may omit it;
    /// missing data is distinct from an explicit assessment of `undelivered`.
    #[serde(
        default,
        alias = "end_to_end_ownership",
        skip_serializing_if = "Option::is_none"
    )]
    pub delivery_state: Option<DeliveryState>,
    /// Every distinct `delivery_state` this goal has carried, oldest first.
    /// Tool-maintained; model-supplied values are ignored.
    #[serde(
        default,
        alias = "end_to_end_ownership_history",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub delivery_state_history: Vec<DeliveryState>,
    /// What verification has actually been performed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_state: Option<VerificationState>,
    /// Distinct verification states, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification_state_history: Vec<VerificationState>,
    /// Intrinsic difficulty of this goal. Descriptive, never gated: it only
    /// calibrates how much delivery follow-through a completion review expects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// How far beyond the literal request the work extended. Completion is
    /// gated at `necessary_followthrough` so consequential adjacent work is not
    /// silently left to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    /// How far an iterative feedback loop has progressed and, at completion,
    /// the semantic basis for stopping. This is distinct from loop quality: a
    /// perfectly measurable loop may still be actively improving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_maturity: Option<IterationMaturity>,
    /// Evidence that an open-ended search has reached a defensible stopping
    /// point, such as a plateau across distinct approaches, exhausted
    /// hypotheses, or an explicit budget limit. Required only when a
    /// research or open-ended goal is marked complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopping_evidence: Option<String>,
    /// Why this goal is impossible to complete. The only way to stop
    /// continuation for unfinished work that is neither waiting nor awaiting
    /// user authorization. `None` means the goal is still being pursued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cannot_complete: Option<String>,
}

impl Serialize for TodoGoal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TodoGoal::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for TodoGoal {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Inspect the old labels before the field deserializer collapses them.
        let raw = serde_json::Value::deserialize(deserializer)?;
        let legacy_checked = |v: &serde_json::Value| {
            v.as_str()
                .is_some_and(|s| s.trim().eq_ignore_ascii_case("workflow_validated"))
        };
        let current_checked = raw
            .get("delivery_state")
            .or_else(|| raw.get("end_to_end_ownership"))
            .is_some_and(legacy_checked);
        let history_checked = raw
            .get("delivery_state_history")
            .or_else(|| raw.get("end_to_end_ownership_history"))
            .and_then(|v| v.as_array())
            .is_some_and(|v| v.iter().any(legacy_checked));
        let mut goal = TodoGoal::deserialize(raw).map_err(serde::de::Error::custom)?;
        if current_checked && goal.verification_state.is_none() {
            goal.verification_state = Some(VerificationState::CheckedEndToEnd);
        }
        if history_checked
            && !goal
                .verification_state_history
                .contains(&VerificationState::CheckedEndToEnd)
        {
            goal.verification_state_history
                .insert(0, VerificationState::CheckedEndToEnd);
        }
        let mut seen = std::collections::HashSet::new();
        goal.delivery_state_history.retain(|s| seen.insert(*s));
        Ok(goal)
    }
}

/// A goal field changed by a todo-tool update. This lets transcript renderers
/// show a concise quality-gate refinement instead of repeating the full plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoGoalField {
    #[serde(alias = "hill_climbability")]
    ClosedFeedbackLoop,
    FeedbackLoop,
    FeedbackLoopRelevance,
    FeedbackLoopCoverage,
    FeedbackLoopTraceability,
    #[serde(alias = "end_to_end_ownership")]
    DeliveryState,
    VerificationState,
    Autonomy,
    IterationMaturity,
    StoppingEvidence,
    CannotComplete,
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

#[cfg(test)]
mod semantic_state_tests {
    use super::*;

    #[test]
    fn todo_waiting_on_is_backward_compatible_and_nullable() {
        let old =
            serde_json::json!({"id":"a", "content":"A", "status":"pending", "priority":"high"});
        let todo: TodoItem = serde_json::from_value(old.clone()).unwrap();
        assert!(todo.waiting_on.is_empty());
        assert!(
            serde_json::to_value(&todo)
                .unwrap()
                .get("waiting_on")
                .is_none()
        );
        for value in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!(["task-a"]),
        ] {
            let mut payload = old.clone();
            payload["waiting_on"] = value.clone();
            let todo: TodoItem = serde_json::from_value(payload).unwrap();
            assert_eq!(
                todo.waiting_on,
                if value.is_null() {
                    vec![]
                } else {
                    serde_json::from_value::<Vec<String>>(value).unwrap()
                }
            );
        }
        for invalid in [
            serde_json::json!(42),
            serde_json::json!([null]),
            serde_json::json!({"task": "a"}),
        ] {
            let mut payload = old.clone();
            payload["waiting_on"] = invalid;
            assert!(serde_json::from_value::<TodoItem>(payload).is_err());
        }
    }

    #[test]
    fn legacy_item_blockers_are_ignored_on_load() {
        let todo: TodoItem = serde_json::from_value(serde_json::json!({
            "id": "a", "content": "A", "status": "pending", "priority": "high",
            "external_blocker": {"reason": "old", "review_at": "2020-01-01T00:00:00Z"},
            "completely_blocked": {"reason": "old"}
        }))
        .unwrap();
        assert_eq!(todo.requires_user_auth, None);
        let saved = serde_json::to_value(&todo).unwrap();
        assert!(saved.get("external_blocker").is_none());
        assert!(saved.get("completely_blocked").is_none());
        assert!(
            serde_json::from_value::<TodoUserAuth>(serde_json::json!({"reason": " "})).is_err()
        );
    }

    #[test]
    fn states_serialize_as_snake_case_strings() {
        assert_eq!(
            serde_json::to_value(IntentUnderstanding::Clear).unwrap(),
            "clear"
        );
        assert_eq!(
            serde_json::to_value(DeliveryState::Delivered).unwrap(),
            "delivered"
        );
        assert_eq!(
            serde_json::to_value(Difficulty::OpenEnded).unwrap(),
            "open_ended"
        );
        assert_eq!(
            serde_json::to_value(FeedbackLoopRelevance::AcceptanceAligned).unwrap(),
            "acceptance_aligned"
        );
        assert_eq!(
            serde_json::to_value(FeedbackLoopRelevance::Synthetic).unwrap(),
            "synthetic"
        );
        assert_eq!(
            serde_json::to_value(FeedbackLoopRelevance::AcceptanceBlocked).unwrap(),
            "acceptance_blocked"
        );
        assert_eq!(
            serde_json::to_value(FeedbackLoopCoverage::EdgeAndIntegrationPaths).unwrap(),
            "edge_and_integration_paths"
        );
        assert_eq!(
            serde_json::to_value(Autonomy::NecessaryFollowthrough).unwrap(),
            "necessary_followthrough"
        );
    }

    #[test]
    fn states_round_trip_and_parse_case_insensitively() {
        for state in [
            FeedbackLoopState::Absent,
            FeedbackLoopState::Weak,
            FeedbackLoopState::Usable,
            FeedbackLoopState::Strong,
            FeedbackLoopState::Closed,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(
                serde_json::from_str::<FeedbackLoopState>(&json).unwrap(),
                state
            );
            assert_eq!(FeedbackLoopState::parse(state.as_str()), Some(state));
            assert_eq!(
                FeedbackLoopState::parse(&state.as_str().to_ascii_uppercase()),
                Some(state)
            );
        }
    }

    #[test]
    fn states_reject_invalid_and_numeric_values() {
        assert!(serde_json::from_str::<ConfidenceState>("\"bogus\"").is_err());
        assert!(serde_json::from_str::<ConfidenceState>("85").is_err());
        assert!(serde_json::from_str::<ConfidenceState>("\"85\"").is_err());
        assert!(serde_json::from_str::<IntentUnderstanding>("\"complete\"").is_err());
        assert!(serde_json::from_str::<IntentUnderstanding>("97").is_err());
    }

    #[test]
    fn state_ordering_and_levels_match() {
        assert!(IntentUnderstanding::Uncertain < IntentUnderstanding::Partial);
        assert!(IntentUnderstanding::Partial < IntentUnderstanding::Clear);
        assert!(ConfidenceState::Speculative < ConfidenceState::Verified);
        assert!(DeliveryState::Undelivered < DeliveryState::PartiallyDelivered);
        assert!(DeliveryState::PartiallyDelivered < DeliveryState::Delivered);
        assert!(FeedbackLoopRelevance::Indirect < FeedbackLoopRelevance::Synthetic);
        assert!(FeedbackLoopRelevance::Synthetic < FeedbackLoopRelevance::Representative);
        assert!(FeedbackLoopRelevance::Representative < FeedbackLoopRelevance::AcceptanceBlocked);
        assert!(
            FeedbackLoopRelevance::AcceptanceBlocked < FeedbackLoopRelevance::AcceptanceAligned
        );
        for state in [
            ConfidenceState::Speculative,
            ConfidenceState::Plausible,
            ConfidenceState::Validated,
            ConfidenceState::Verified,
        ] {
            assert_eq!(ConfidenceState::from_level(state.level()), state);
        }
    }

    #[test]
    fn goal_serializes_new_field_names_only() {
        let goal = TodoGoal {
            delivery_state: Some(DeliveryState::Undelivered),
            difficulty: Some(Difficulty::Involved),
            autonomy: Some(Autonomy::Proactive),
            ..Default::default()
        };
        let json = serde_json::to_value(&goal).unwrap();
        assert_eq!(json["delivery_state"], "undelivered");
        assert_eq!(json["difficulty"], "involved");
        assert_eq!(json["autonomy"], "proactive");
        assert!(json.get("end_to_end_ownership").is_none());
    }

    #[test]
    fn legacy_delivery_and_verification_migrate_independently() {
        for (label, delivery, verification) in [
            ("not_started", DeliveryState::Undelivered, None),
            ("change_made", DeliveryState::Undelivered, None),
            ("integrated", DeliveryState::Undelivered, None),
            (
                "workflow_validated",
                DeliveryState::Undelivered,
                Some(VerificationState::CheckedEndToEnd),
            ),
            ("outcome_delivered", DeliveryState::Delivered, None),
        ] {
            let goal: TodoGoal =
                serde_json::from_value(serde_json::json!({ "end_to_end_ownership": label }))
                    .unwrap();
            assert_eq!(
                (goal.delivery_state, goal.verification_state),
                (Some(delivery), verification)
            );
            assert!(DeliveryState::parse(label).is_none()); // legacy input is read-only
        }
        let goal: TodoGoal = serde_json::from_value(serde_json::json!({
            "delivery_state": "workflow_validated",
            "verification_state": "checked_in_isolation",
            "end_to_end_ownership_history": ["not_started", "integrated", "workflow_validated", "outcome_delivered"],
            "verification_state_history": ["checked_in_isolation"]
        })).unwrap();
        assert_eq!(
            goal.verification_state,
            Some(VerificationState::CheckedInIsolation)
        );
        assert_eq!(
            goal.delivery_state_history,
            [DeliveryState::Undelivered, DeliveryState::Delivered]
        );
        assert_eq!(
            goal.verification_state_history,
            [
                VerificationState::CheckedEndToEnd,
                VerificationState::CheckedInIsolation
            ]
        );
        let saved = serde_json::to_string(&goal).unwrap();
        assert_eq!(serde_json::from_str::<TodoGoal>(&saved).unwrap(), goal);
        assert!(!saved.contains("workflow_validated"));
        assert_eq!(
            serde_json::from_str::<TodoGoal>("{}").unwrap(),
            TodoGoal::default()
        );
    }

    #[test]
    fn legacy_goal_field_alias_deserializes() {
        let field: TodoGoalField = serde_json::from_str("\"end_to_end_ownership\"").unwrap();
        assert_eq!(field, TodoGoalField::DeliveryState);
    }
}
