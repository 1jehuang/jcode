# Semantic Todo Assessment Migration Spec (worker handoff)

Goal: remove all 0-100 quality scores from the todo system. Replace with semantic
string enums. Add difficulty, autonomy, delivery_state. Legacy numeric sessions
must still load (numbers map to enums on deserialize). Every todo write must
always persist (never reject a write; gates only emit continuations). Difficulty
and autonomy are NEVER gated. Completion gates evaluate confidence (evidence) and
delivery_state, with difficulty only calibrating how strict the delivery bar is.

## Enums (all: snake_case string serde, Ord by declaration order, in jcode-task-types)

```rust
pub enum IntentUnderstanding { Uncertain, Partial, Clear, Complete }
pub enum FeedbackLoopState  { Absent, Weak, Usable, Strong, Closed }
pub enum ConfidenceState    { Speculative, Plausible, Validated, Verified }
pub enum Difficulty { Trivial, Routine, Involved, Complex, Hard, Expert, Research, OpenEnded }
pub enum Autonomy   { RequestedOnly, NecessaryFollowthrough, Proactive, Stewardship }
pub enum DeliveryState { ChangeMade, Integrated, WorkflowValidated, OutcomeDelivered }
```

Each enum gets:
- `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ...)]`, `#[serde(rename_all = "snake_case")]`
- `pub fn as_str(&self) -> &'static str`
- `pub fn parse(&str) -> Option<Self>` (trim, ascii-lowercase)
- `pub fn from_legacy_score(u8) -> Self` per the tables below
- Deserialization must accept BOTH a string variant AND a legacy integer
  (use a custom `Deserialize` impl or `#[serde(deserialize_with)]` helper via
  untagged number-or-string). Serialization is always the string.

## Legacy numeric -> enum mapping

- IntentUnderstanding: 0-59 Uncertain, 60-95 Partial, 96-99 Clear, 100 Complete
- FeedbackLoopState: 0-19 Absent, 20-49 Weak, 50-79 Usable, 80-95 Strong, 96-100 Closed
- ConfidenceState: 0-59 Speculative, 60-95 Plausible, 96-99 Validated, 100 Verified
- DeliveryState (from legacy end_to_end_ownership): 0-49 ChangeMade, 50-79 Integrated,
  80-95 WorkflowValidated, 96-100 OutcomeDelivered
- Difficulty / Autonomy: no legacy field; default None.

## Field changes

TodoItem:
- `confidence: Option<ConfidenceState>` (was Option<u8>)
- `completion_confidence: Option<ConfidenceState>`
- `confidence_history: Vec<ConfidenceState>` (legacy Vec<u8> entries convert on load)
- NEW `difficulty: Option<Difficulty>` (skip_serializing_if none)

TodoPlan:
- `understands_user_intent: Option<IntentUnderstanding>` (keep aliases alignment_score / user_intention_alignment)
- `understands_user_intent_history: Vec<IntentUnderstanding>`

TodoGoal:
- `closed_feedback_loop: Option<FeedbackLoopState>` (keep hill_climbability alias) + history
- `end_to_end_ownership` RENAMED to `delivery_state: Option<DeliveryState>`
  with `#[serde(alias = "end_to_end_ownership")]`; history field
  `delivery_state_history` with alias `end_to_end_ownership_history`
- NEW `difficulty: Option<Difficulty>`
- NEW `autonomy: Option<Autonomy>`
- `feedback_loop: Option<String>` unchanged

GateObservation.score becomes the relevant enum? Keep it simple:
change `score: Option<u8>` to a semantic snapshot; simplest is
`state: Option<String>` holding as_str, with `#[serde(alias = "score")]`
tolerating old numeric via number-or-string deserializer. Workers may instead
keep two optional fields; prefer minimal churn.

## Gate semantics (jcode-base todo.rs, app-core, tui)

Never reject a write. All existing "deferred observation + turn-end digest +
continuation" plumbing stays, only comparisons change:

- Intent gate: passing when `understands_user_intent >= Clear`.
  Severe first-write nudge when `== Uncertain` (replaces SEVERE_INTENT_MISUNDERSTANDING).
- Feedback loop gate: passing when `closed_feedback_loop >= Closed` (i.e. == Closed).
- Completion confidence gate: completed todo passes when
  `completion_confidence >= Validated`.
- Confidence spike: a completed todo is spike-finished when its final history
  step jumps 2 or more levels (e.g. Speculative -> Validated), or with no
  history when completion is >= 2 levels above planning confidence.
- Delivery gate (replaces ownership gate): a completed group passes when its
  goal's `delivery_state` meets the required bar:
  - difficulty None | Trivial | Routine  -> WorkflowValidated or better
  - Involved and above                   -> OutcomeDelivered
  Difficulty itself is never a gate: absent difficulty just uses the lenient bar.
- Autonomy: never gated anywhere. Display/telemetry only.

Delete or replace numeric constants (QUALITY_GATE_THRESHOLD, LOW_*,
SEVERE_INTENT_MISUNDERSTANDING, TODO_CONFIDENCE_SPIKE) with enum-based
predicates exported from jcode-base::todo, e.g.:
`intent_understanding_passes`, `feedback_loop_passes`,
`completion_confidence_passes`, `delivery_state_passes(goal)`,
`required_delivery_state(difficulty)`.
TUI code referencing the old constants must use these predicates.

## Tool schema (app-core todo.rs)

- Replace integer 0-100 properties with string enums:
  - todo item: `confidence`, `completion_confidence` -> enum of
    speculative|plausible|validated|verified; NEW optional `difficulty` enum.
  - plan: `understands_user_intent` -> uncertain|partial|clear|complete.
  - goal: `closed_feedback_loop` -> absent|weak|usable|strong|closed;
    `end_to_end_ownership` REPLACED by `delivery_state` ->
    change_made|integrated|workflow_validated|outcome_delivered;
    NEW optional `difficulty` and `autonomy` enums.
- normalize_todo_input: keep string coercion; numeric values (int, float,
  numeric string) for any of these fields must be converted to the mapped
  enum string so legacy transcripts/providers still parse. Empty string -> null.
- Histories remain tool-maintained; record_score_observation generalizes over
  the enums (push when last != new).
- Telemetry: TelemetryScoreSummary stays u8-based; map each enum to a
  representative score via `legacy_score()` on each enum:
  - IntentUnderstanding: 40/80/96/100
  - FeedbackLoopState: 10/35/65/88/98
  - ConfidenceState: 40/80/96/100
  - DeliveryState: 25/65/88/98
  This keeps jcode-telemetry-core, usage-types, and the worker schema untouched.

## Always-save requirement

Verify no code path returns Err/rejects before `save_todos/save_goals/save_plan`
based on assessment values. `newly_completed_groups_have_sufficient_ownership`
(write-time variant) should be removed or kept only for turn-finish; nothing may
block persisting.

## Testing

- task-types: round-trip each enum; legacy numeric JSON deserializes (e.g.
  `{"understands_user_intent": 97}` -> Clear); serializes as string.
- base: update gate tests to enum values; legacy alias fields still load.
- app-core: schema test (no digits/0-100 in model-visible schema besides none),
  normalize coercion of numeric legacy input, merge histories, always-save.
- tui: update todos_view/ui_messages/info_widget tests to render state words.
- Commands: `cargo test -p jcode-task-types -p jcode-base`,
  `cargo test -p jcode-app-core todo`, `cargo test -p jcode-tui todo`,
  `cargo check -p jcode-tui -p jcode-telemetry-core`.

Do NOT touch unrelated dirty files. Commit nothing;
the coordinator commits.
