//! Three-judge median pipeline.
//!
//! Each agent diff is graded by **three** frontier models in parallel
//! (planned slate: `gpt-5`, `gemini-pro`, `claude-sonnet`); the median
//! `overall_score` selects which judge's qualitative analysis is
//! reported, while the per-dimension scores are averaged across all
//! valid judges. This mirrors the design of BuffBench's
//! `judgeCommitResult` in `/tmp/codebuff/evals/buffbench/judge.ts`.
//!
//! The actual provider plumbing (which talks to each judge model
//! through the existing jcode provider registry) lands in Phase 5.4.
//! Until then both entry points are `unimplemented!()` stubs whose
//! signatures define the public surface the rest of the harness will
//! depend on.

use std::collections::HashMap;

use anyhow::Result;

use crate::types::{EvalCommit, JudgingResult};

/// Judge an agent's diff against the ground truth using three models in
/// parallel and return a [`JudgingResult`] whose qualitative analysis
/// comes from the median judge and whose numeric scores are averaged
/// across all judges that returned successfully.
///
/// Why median + average?
/// - **Median analysis** picks a representative voice and avoids the
///   outlier judge dominating the prose.
/// - **Average scores** smooth out judge-specific bias so the canonical
///   overall metric tracks consensus, not whichever model happened to
///   be selected.
///
/// Design source: `/tmp/codebuff/evals/buffbench/judge.ts`
/// (`judgeCommitResult`).
///
/// `context_files` is a `path -> contents` map of supplemental files
/// from the parent commit; the judges receive these inline in the
/// prompt to ground their evaluation.
pub async fn judge_with_three_models(
    commit: &EvalCommit,
    agent_diff: &str,
    context_files: &HashMap<String, String>,
) -> Result<JudgingResult> {
    let _ = (commit, agent_diff, context_files);
    unimplemented!("Phase 5.4: run gpt-5 / gemini-pro / sonnet judges in parallel and return median+average")
}

/// Invoke a single judge model with a fully-rendered prompt.
///
/// Used internally by [`judge_with_three_models`] and exposed publicly
/// so callers can re-judge a stored run with a different model without
/// re-running the full three-judge pipeline.
///
/// Design source: `/tmp/codebuff/evals/buffbench/judge.ts`
/// (`runSingleJudge`).
pub async fn run_single_judge(model_id: &str, prompt: &str) -> Result<JudgingResult> {
    let _ = (model_id, prompt);
    unimplemented!("Phase 5.4: wire to provider registry")
}
