//! Spawn a jcode agent inside a freshly-prepared repo clone, run a
//! single eval task, and capture the resulting diff and trace.
//!
//! The runner resolves the configured `agent_id` through the
//! [`jcode_agent_runtime::AgentRegistry`] (loaded from
//! `.jcode/agents/*.toml`), spawns the binary as a subprocess in the
//! repo working directory, streams the trace, and finally extracts the
//! unified diff against the parent commit.
//!
//! Design source: `/tmp/codebuff/evals/buffbench/agent-runner.ts`.
//!
//! Implementation lands in Phase 5.3; for now both entry points are
//! `unimplemented!()` stubs whose signatures fix the contract the rest
//! of the harness will rely on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::types::EvalRun;

/// Configuration for a single agent evaluation run.
///
/// `repo_path` should already contain a clean checkout of the eval
/// commit's parent SHA; the runner does not clone for the caller.
#[derive(Debug, Clone)]
pub struct AgentRunConfig {
    /// ID of the agent to run, matching an entry in the
    /// `jcode-agent-runtime` registry.
    pub agent_id: String,
    /// Natural-language prompt to send to the agent (typically
    /// `EvalCommit::prompt`).
    pub prompt: String,
    /// Working directory containing the prepared repo at the parent
    /// commit.
    pub repo_path: PathBuf,
    /// Hard cap on the number of agent turns before the run is
    /// aborted; mirrors BuffBench's per-task turn budget.
    pub max_turns: u32,
    /// Extra environment variables applied to the agent subprocess on
    /// top of the calling process's environment.
    pub env: HashMap<String, String>,
}

/// Spawn the configured agent in `config.repo_path`, run it to
/// completion (or the turn / time budget), and return an [`EvalRun`]
/// populated with the agent's diff, judging placeholder, cost, and
/// duration.
///
/// The runner is responsible for:
/// - Capturing the agent's full trace for later analysis.
/// - Calling [`extract_diff_from_repo`] once the agent finishes.
/// - Invoking the judging pipeline (or leaving that to the caller —
///   the final wiring is decided in Phase 5.3).
pub async fn run_agent_in_repo(config: AgentRunConfig) -> Result<EvalRun> {
    let _ = config;
    unimplemented!("Phase 5.3: spawn jcode subprocess in repo, capture trace")
}

/// Produce a unified diff describing all uncommitted changes in
/// `repo_path` against its currently-checked-out HEAD.
///
/// Used after the agent finishes editing to capture the "agent's
/// changes" half of the judging input. The exact git invocation
/// (likely `git diff --no-color HEAD`) is finalized in Phase 5.3.
pub fn extract_diff_from_repo(repo_path: &Path) -> Result<String> {
    let _ = repo_path;
    unimplemented!("Phase 5.3: shell out to git diff and return the unified diff")
}
