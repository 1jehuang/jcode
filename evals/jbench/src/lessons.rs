//! Lessons extractor.
//!
//! After an eval run finishes, the lessons extractor compares the
//! agent's actual diff and trace against the ground-truth diff and
//! distills a small list of [`Lesson`]s describing what went wrong and
//! what the agent should have done instead. These can be appended to a
//! per-agent lessons file and folded back into the agent's system
//! prompt or memory graph.
//!
//! Design source: `/tmp/codebuff/evals/buffbench/lessons-extractor.ts`.
//!
//! Implementation lands in Phase 5.5.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// One distilled lesson from a single eval run.
///
/// Kept deliberately minimal — both fields are free-form prose. Richer
/// structure (severity, tags, links to specific commits) can be added
/// later without breaking the on-disk format because lesson files are
/// JSON arrays of this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    /// Concise description of the failure mode observed in the trace
    /// or diff. One or two sentences.
    pub what_went_wrong: String,
    /// Concise description of the corrective behavior the agent should
    /// have performed instead. One or two sentences.
    pub what_should_have_been_done: String,
}

/// Run the lessons-extractor judge over a finished eval run and return
/// zero or more [`Lesson`]s.
///
/// The extractor receives the prompt the agent was given, the ground
/// truth diff for context, the diff the agent actually produced, and
/// the agent's full trace. It returns an empty `Vec` when the run was
/// successful enough that no corrective lesson applies.
pub async fn extract_lessons(
    prompt: &str,
    ground_truth_diff: &str,
    agent_diff: &str,
    agent_trace: &str,
) -> Result<Vec<Lesson>> {
    let _ = (prompt, ground_truth_diff, agent_diff, agent_trace);
    unimplemented!("Phase 5.5: invoke lessons-extractor judge and parse Vec<Lesson>")
}

/// Append `lessons` to the per-agent lessons file at
/// `lessons_dir/<agent_id>.json`, creating the file (and the directory)
/// if needed.
///
/// The on-disk format is a JSON array of [`Lesson`]; appending preserves
/// previously-extracted lessons so the file accumulates over many runs.
pub fn append_lessons_to_file(
    agent_id: &str,
    lessons: &[Lesson],
    lessons_dir: &Path,
) -> Result<()> {
    let _ = (agent_id, lessons, lessons_dir);
    unimplemented!("Phase 5.5: read-modify-write JSON array at lessons_dir/<agent_id>.json")
}
