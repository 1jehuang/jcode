//! Serializable data types modeling JBench's eval inputs and outputs.
//!
//! These types are direct Rust analogues of BuffBench's TypeScript types
//! (`/tmp/codebuff/evals/buffbench/types.ts`) with one deliberate
//! deviation: every field uses `snake_case` in both the Rust definition
//! and the on-disk JSON form, because the rest of jcode's serialized
//! formats already follow `snake_case`.
//!
//! All public types derive `Debug`, `Clone`, `Serialize`, and
//! `Deserialize`. Numeric scores are `f64` in the `[0.0, 10.0]` range —
//! validation is not enforced at the type level so partial / in-progress
//! results round-trip cleanly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Status of a single file inside an [`EvalCommit`]'s diff.
///
/// Mirrors BuffBench's `'modified' | 'added' | 'deleted' | 'renamed'`
/// string union; serialized as lowercase strings so generated eval JSON
/// stays compact and readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileDiffStatus {
    /// File existed before and after, with content changes.
    Modified,
    /// File was created in this commit.
    Added,
    /// File was deleted in this commit.
    Deleted,
    /// File was renamed (and possibly modified) in this commit.
    Renamed,
}

/// Per-file diff entry for a single eval commit.
///
/// `old_path` is populated only for `Renamed` entries; for all other
/// statuses it is `None` and skipped during serialization to keep the
/// JSON output compact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    /// Current path of the file (post-commit). For renames this is the
    /// new name.
    pub path: String,
    /// What kind of change this file underwent.
    pub status: FileDiffStatus,
    /// Previous path, only populated when `status == Renamed`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_path: Option<String>,
    /// Unified diff text for the change. May be empty for pure renames.
    pub diff: String,
}

/// One eval task: a single git commit reconstructed from its parent.
///
/// The agent under test starts from `parent_sha`, is given `prompt`,
/// and is judged against `file_diffs`. `supplemental_files` lists
/// additional context paths the harness should preload into the agent's
/// view (BuffBench picks these via a separate filter step).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCommit {
    /// Stable identifier for the task, typically `<short_sha>-<slug>`.
    pub id: String,
    /// Target commit SHA — the ground-truth state.
    pub sha: String,
    /// Parent commit SHA — the starting state for the agent.
    pub parent_sha: String,
    /// Technical specification distilled from the commit message.
    pub spec: String,
    /// Natural-language prompt presented to the agent under test.
    pub prompt: String,
    /// Extra files (relative paths) the harness should expose as
    /// context, in addition to whatever the agent fetches itself.
    pub supplemental_files: Vec<String>,
    /// Ground-truth file diffs for this commit.
    pub file_diffs: Vec<FileDiff>,
}

/// Top-level eval data file (v2 schema), produced by `gen-evals` and
/// consumed by `run`.
///
/// `env` and `final_check_commands` are reserved for future use by the
/// runner; they are part of the on-disk schema today so eval JSON files
/// authored against this scaffold remain forward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalDataV2 {
    /// Source repository to clone for each task.
    pub repo_url: String,
    /// Optional override for the local clone directory name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub test_repo_name: Option<String>,
    /// ISO-8601 timestamp of when this eval file was generated.
    pub generation_date: String,
    /// Optional one-time setup command (e.g. `npm install`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub init_command: Option<String>,
    /// Environment variables to apply when running agents and final
    /// checks. Defaults to empty.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Validation commands run after the agent finishes (e.g. `cargo
    /// test`). Defaults to empty.
    #[serde(default)]
    pub final_check_commands: Vec<String>,
    /// The actual list of commits to evaluate against.
    pub eval_commits: Vec<EvalCommit>,
}

/// Output of a single judge invocation (or the median of three).
///
/// All three score fields are on the same `[0.0, 10.0]` scale; `f64` is
/// used so we can also store the *averaged* per-dimension scores when
/// aggregating multiple judges (see `judge::judge_with_three_models`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgingResult {
    /// Free-form prose comparing the agent's diff to the ground truth.
    pub analysis: String,
    /// Bullet-point strengths called out by the judge.
    pub strengths: Vec<String>,
    /// Bullet-point weaknesses called out by the judge.
    pub weaknesses: Vec<String>,
    /// How completely the prompt was addressed, `[0.0, 10.0]`.
    pub completion_score: f64,
    /// Code structure / maintainability, `[0.0, 10.0]`.
    pub code_quality_score: f64,
    /// Combined assessment, `[0.0, 10.0]`. JBench's canonical metric.
    pub overall_score: f64,
}

/// Outcome of running one agent on one eval commit.
///
/// `error` is `Some` when the agent crashed, timed out, or otherwise
/// failed to produce a usable diff; in that case `judging` will
/// typically contain a zero-scored placeholder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    /// SHA of the eval commit this run targeted.
    pub commit_sha: String,
    /// Prompt the agent was given.
    pub prompt: String,
    /// Unified diff produced by the agent against the parent commit.
    pub diff: String,
    /// Three-judge result (see [`crate::judge`]).
    pub judging: JudgingResult,
    /// Estimated USD cost of running the agent.
    pub cost_usd: f64,
    /// Wall-clock duration of the run in milliseconds.
    pub duration_ms: u64,
    /// Populated when the run failed to complete cleanly.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

/// Aggregated results for one agent across an entire eval suite.
///
/// `average_score` here is `overall_score`; cost and duration averages
/// are computed across **all** runs (including failures) so consumers
/// can spot agents that are cheap or fast at the price of correctness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvalResults {
    /// ID of the agent (matches an `AgentDefinition::id` in the
    /// `jcode-agent-runtime` registry).
    pub agent_id: String,
    /// Per-commit runs, in evaluation order.
    pub runs: Vec<EvalRun>,
    /// Mean of `judging.overall_score` across runs.
    pub average_score: f64,
    /// Mean of `cost_usd` across runs.
    pub average_cost: f64,
    /// Mean of `duration_ms` across runs.
    pub average_duration_ms: u64,
}
