//! Issue #59: closed-loop skill distillation.
//!
//! Primitive for capturing recurring agent workflows as reusable
//! skill drafts. The full pipeline (cluster recurring patterns,
//! synthesize SKILL.md candidates, dedupe against existing skills,
//! propose for user review) is multi-stage; this module ships the
//! **storage primitive** + **draft schema** + **append API** so
//! agents can record workflow snapshots that the distiller will
//! later consume.
//!
//! ## Storage
//!
//! Drafts live at `<JCODE_HOME>/skill_drafts.jsonl` — newline-
//! delimited JSON. Each line:
//!
//! ```json
//! {
//!   "id": "uuid-v4",
//!   "captured_at": "2026-05-24T11:00:00Z",
//!   "session_id": "blue-fox-1234",
//!   "trigger_summary": "user asked to refactor a Rust module",
//!   "tool_sequence": ["read", "agentgrep", "edit", "edit", "bash"],
//!   "outcome": "success",
//!   "user_signal": null
//! }
//! ```
//!
//! ## API
//!
//! ```rust,no_run
//! use jcode::skill_distillation::{record_workflow, WorkflowDraft};
//!
//! record_workflow(WorkflowDraft {
//!     session_id: "abc-123".to_string(),
//!     trigger_summary: "Wrote a new skill file".to_string(),
//!     tool_sequence: vec!["write".to_string(), "edit".to_string()],
//!     outcome: "success".to_string(),
//!     user_signal: None,
//! })?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! ## Out of scope (this PR)
//!
//! - Pattern clustering (which sequences recur often enough to
//!   warrant skillification)
//! - SKILL.md synthesis (LLM call to draft a skill from a cluster)
//! - Drift detection vs existing skills
//! - User-facing review UI (`jcode skills propose`)

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDraft {
    /// Session that produced this workflow. Used by the distiller
    /// to fold same-session patterns together.
    pub session_id: String,
    /// One-sentence summary of what the user wanted. Should not
    /// echo PII or secrets — caller is responsible for sanitizing.
    pub trigger_summary: String,
    /// The ordered list of tool names invoked during this workflow.
    /// Limit to ~50 entries to keep the storage line size sane.
    pub tool_sequence: Vec<String>,
    /// `success` | `failure` | `cancelled` | `unknown`.
    pub outcome: String,
    /// Optional user feedback signal, e.g. `thumbs_up`, `thumbs_down`,
    /// `re_used` (user invoked again with similar intent shortly
    /// after). `None` means no signal recorded.
    pub user_signal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredWorkflow {
    id: String,
    captured_at: DateTime<Utc>,
    #[serde(flatten)]
    draft: WorkflowDraft,
}

fn drafts_path() -> Result<PathBuf> {
    Ok(jcode_storage::jcode_dir()?.join("skill_drafts.jsonl"))
}

/// Append a workflow draft to the persistent JSONL store. Idempotency
/// is the caller's responsibility — duplicates are not deduplicated.
///
/// Caps the stored `tool_sequence` at 100 entries to bound the
/// per-line size; longer sequences are truncated with the marker
/// `"...truncated"` in the last slot.
pub fn record_workflow(mut draft: WorkflowDraft) -> Result<()> {
    if draft.tool_sequence.len() > 100 {
        draft.tool_sequence.truncate(99);
        draft.tool_sequence.push("...truncated".to_string());
    }

    let path = drafts_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    let entry = StoredWorkflow {
        id: short_random_id(),
        captured_at: Utc::now(),
        draft,
    };
    let line = serde_json::to_string(&entry).context("serialize workflow draft")?;

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {} for append", path.display()))?;
    writeln!(file, "{line}").context("append workflow draft line")?;
    Ok(())
}

/// Read all stored drafts. Used by the future distiller; for now
/// also useful for tests + a `jcode skills drafts` debug subcommand.
pub fn load_workflows() -> Result<Vec<WorkflowDraft>> {
    let path = drafts_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<StoredWorkflow>(line) {
            Ok(stored) => out.push(stored.draft),
            Err(err) => {
                eprintln!("jcode skill_distillation: skip malformed line: {err}");
            }
        }
    }
    Ok(out)
}

/// Count of stored drafts. Cheap probe used by the (future) distiller
/// scheduler to decide when to run a clustering pass.
pub fn count_workflows() -> Result<usize> {
    Ok(load_workflows()?.len())
}

fn short_random_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos & 0xffff_ffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_isolated_home<F, T>(f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        let result = f();
        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        result
    }

    fn sample_draft() -> WorkflowDraft {
        WorkflowDraft {
            session_id: "blue-fox-1234".to_string(),
            trigger_summary: "test workflow".to_string(),
            tool_sequence: vec!["read".to_string(), "edit".to_string()],
            outcome: "success".to_string(),
            user_signal: None,
        }
    }

    #[test]
    fn record_then_load_round_trips() {
        with_isolated_home(|| {
            record_workflow(sample_draft()).unwrap();
            let drafts = load_workflows().unwrap();
            assert_eq!(drafts.len(), 1);
            assert_eq!(drafts[0].session_id, "blue-fox-1234");
            assert_eq!(drafts[0].tool_sequence, vec!["read", "edit"]);
        });
    }

    #[test]
    fn count_matches_record_calls() {
        with_isolated_home(|| {
            assert_eq!(count_workflows().unwrap(), 0);
            record_workflow(sample_draft()).unwrap();
            record_workflow(sample_draft()).unwrap();
            record_workflow(sample_draft()).unwrap();
            assert_eq!(count_workflows().unwrap(), 3);
        });
    }

    #[test]
    fn long_tool_sequence_is_truncated() {
        with_isolated_home(|| {
            let mut draft = sample_draft();
            draft.tool_sequence = (0..200).map(|i| format!("tool_{i}")).collect();
            record_workflow(draft).unwrap();
            let drafts = load_workflows().unwrap();
            assert_eq!(drafts[0].tool_sequence.len(), 100);
            assert_eq!(drafts[0].tool_sequence.last().unwrap(), "...truncated");
        });
    }

    #[test]
    fn malformed_lines_are_skipped_with_warning() {
        with_isolated_home(|| {
            let path = drafts_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{not json}\n{also not json}\n").unwrap();
            record_workflow(sample_draft()).unwrap();
            let drafts = load_workflows().unwrap();
            // The 2 garbage lines were skipped; 1 valid line remains.
            assert_eq!(drafts.len(), 1);
        });
    }

    #[test]
    fn missing_file_returns_empty() {
        with_isolated_home(|| {
            assert!(load_workflows().unwrap().is_empty());
            assert_eq!(count_workflows().unwrap(), 0);
        });
    }

    #[test]
    fn user_signal_round_trips() {
        with_isolated_home(|| {
            let mut draft = sample_draft();
            draft.user_signal = Some("thumbs_up".to_string());
            record_workflow(draft).unwrap();
            let drafts = load_workflows().unwrap();
            assert_eq!(drafts[0].user_signal.as_deref(), Some("thumbs_up"));
        });
    }
}
