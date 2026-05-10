use crate::edit_planner::PlannedEdit;
use similar::{ChangeTag, TextDiff};
use tokio::fs;
use std::path::Path;

/// A file that has been read into memory for parallel processing.
#[derive(Debug, Clone)]
pub struct FileBuffer {
    pub path: String,
    pub content: String,
}

/// A file after parallel processing with its diff computed.
#[derive(Debug, Clone)]
pub struct ProcessedFile {
    pub path: String,
    pub original_content: String,
    pub new_content: String,
    pub hunks: Vec<DiffHunk>,
}

/// A single diff hunk from the similar crate.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_end: usize,
    pub new_start: usize,
    pub new_end: usize,
    pub changes: Vec<(ChangeTag, String)>,
}

/// Processes multiple files in parallel using tokio::join!.
pub struct ParallelASTProcessor;

impl ParallelASTProcessor {
    pub fn new() -> Self { Self }

    /// Read all files in parallel, compute diffs for each.
    pub async fn process_parallel(&self, edits: &[PlannedEdit]) -> anyhow::Result<Vec<ProcessedFile>> {
        use futures::future::join_all;
        let futures: Vec<_> = edits.iter().map(|edit| {
            let path = edit.file_path.clone();
            let target_content = edit.new_content.clone();
            async move {
                let content = match fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => String::new(), // new file
                };
                let diff = TextDiff::from_lines(&content, &target_content);
                let hunks: Vec<DiffHunk> = diff
                    .grouped_ops(3)
                    .iter()
                    .map(|ops| {
                        let first = ops.first().unwrap();
                        let last = ops.last().unwrap();
                        let changes: Vec<(ChangeTag, String)> = ops
                            .iter()
                            .flat_map(|op| op.iter_changes())
                            .map(|c| (c.tag(), c.value().to_string()))
                            .collect();
                        DiffHunk {
                            old_start: first.old_index().unwrap_or(0),
                            old_end: last.old_index().unwrap_or(0) + last.len(),
                            new_start: first.new_index().unwrap_or(0),
                            new_end: last.new_index().unwrap_or(0) + last.len(),
                            changes,
                        }
                    })
                    .collect();
                ProcessedFile {
                    path: path.to_string_lossy().to_string(),
                    original_content: content,
                    new_content: target_content,
                    hunks,
                }
            }
        }).collect();
        Ok(join_all(futures).await)
    }
}
