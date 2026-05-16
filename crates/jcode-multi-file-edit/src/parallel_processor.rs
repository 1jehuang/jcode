use crate::edit_planner::PlannedEdit;
use similar::{ChangeTag, TextDiff};

// FileBuffer moved to lib.rs — not used here currently

#[derive(Debug, Clone)]
pub struct ProcessedFile {
    pub path: String, pub original_content: String, pub new_content: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: usize, pub old_end: usize,
    pub new_start: usize, pub new_end: usize,
    pub changes: Vec<(ChangeTag, String)>,
}

pub struct ParallelASTProcessor;

impl ParallelASTProcessor {
    pub fn new() -> Self { Self }

    pub async fn process_parallel(&self, edits: &[PlannedEdit]) -> anyhow::Result<Vec<ProcessedFile>> {
        use futures::future::join_all;
        let futures: Vec<_> = edits.iter().map(|edit| {
            let path = edit.file_path.clone();
            let target_content = edit.new_content.clone();
            async move {
                let content = match tokio::fs::read_to_string(&path).await {
                    Ok(c) => c, Err(_) => String::new(),
                };
                let diff = TextDiff::from_lines(&content, &target_content);
                let mut hunks = Vec::new();
                for group in diff.grouped_ops(3) {
                    if group.is_empty() { continue; }
                    let mut old_s = usize::MAX;
                    let mut old_e = 0usize;
                    let mut new_s = usize::MAX;
                    let mut new_e = 0usize;
                    let mut changes = Vec::new();
                    for op in &group {
                        let o_range = op.old_range();
                        let n_range = op.new_range();
                        old_s = old_s.min(o_range.start);
                        old_e = old_e.max(o_range.end);
                        new_s = new_s.min(n_range.start);
                        new_e = new_e.max(n_range.end);
                        let tag = if o_range.end - o_range.start > 0 && n_range.end - n_range.start > 0 {
                            ChangeTag::Equal
                        } else if o_range.end - o_range.start > 0 {
                            ChangeTag::Delete
                        } else {
                            ChangeTag::Insert
                        };
                        let old_text: String = o_range.map(|i| {
                            content.lines().nth(i).unwrap_or("")
                        }).collect::<Vec<&str>>().join("
");
                        let new_text: String = n_range.map(|i| {
                            target_content.lines().nth(i).unwrap_or("")
                        }).collect::<Vec<&str>>().join("
");
                        if !old_text.is_empty() {
                            changes.push((tag, old_text));
                        } else if !new_text.is_empty() {
                            changes.push((tag, new_text));
                        }
                    }
                    hunks.push(DiffHunk {
                        old_start: old_s, old_end: old_e,
                        new_start: new_s, new_end: new_e,
                        changes,
                    });
                }
                ProcessedFile {
                    path: path.to_string_lossy().to_string(),
                    original_content: content, new_content: target_content, hunks,
                }
            }
        }).collect();
        Ok(join_all(futures).await)
    }
}

impl Default for ParallelASTProcessor {
    fn default() -> Self { Self::new() }
}
