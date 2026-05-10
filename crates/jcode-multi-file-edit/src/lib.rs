//! # jcode-multi-file-edit
//! Composer-style multi-file atomic edit engine.
//!
//! ## Architecture
//!
//! ```text
//! Plan (jcode-plan) → FileSetAnalyzer → FileEditPlanner
//!                                         ↓
//!                                ParallelASTProcessor  ← tokio::join!
//!                                         ↓
//!                                   DiffGenerator     ← similar crate
//!                                         ↓
//!                                   AtomicCommit
//! ```
//!
//! ## Core Flow
//! 1. Accept a Plan from jcode-plan (multi-step, cross-file)
//! 2. Decompose into individual file operations
//! 3. Process files in parallel using tokio::join! — parse AST, compute diffs
//! 4. Merge results into a single unified diff
//! 5. Optionally apply as an atomic commit

mod file_set;
mod parallel_processor;
mod diff_merge;
mod atomic_commit;
mod edit_planner;

pub use file_set::{FileSet, FileOperation, FileEditOp};
pub use parallel_processor::{ParallelASTProcessor, ProcessedFile};
pub use diff_merge::{UnifiedDiff, DiffHunk, merge_diffs};
pub use atomic_commit::{AtomicCommit, CommitResult};
pub use edit_planner::{FileEditPlanner, PlannedEdit};

use std::collections::HashMap;
use similar::{ChangeTag, TextDiff};

/// Composer-style multi-file atomic refactor engine.
pub struct MultiFileEngine {
    planner: FileEditPlanner,
    processor: ParallelASTProcessor,
}

impl MultiFileEngine {
    pub fn new() -> Self {
        Self {
            planner: FileEditPlanner::new(),
            processor: ParallelASTProcessor::new(),
        }
    }

    /// Execute a multi-file edit plan atomically.
    /// 1. Plans are decomposed into file operations
    /// 2. All files are parsed in parallel via tokio::join!
    /// 3. Diffs are computed and merged into a unified result
    pub async fn execute_atomic(&self, files: Vec<FileSet>) -> anyhow::Result<CommitResult> {
        let edits = self.planner.plan(&files)?;
        let processed = self.processor.process_parallel(&edits).await?;
        let unified = merge_diffs(&processed);
        Ok(CommitResult::new(unified, processed))
    }
}

impl Default for MultiFileEngine { fn default() -> Self { Self::new() } }
