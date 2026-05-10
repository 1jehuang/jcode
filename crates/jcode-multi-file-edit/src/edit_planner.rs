use crate::file_set::{FileSet, FileOperation, FileEditOp};
use std::path::PathBuf;

/// A planned edit with final content for each file.
#[derive(Debug, Clone)]
pub struct PlannedEdit {
    pub file_path: PathBuf,
    pub new_content: String,
}

/// Converts FileSets into concrete PlannedEdits.
pub struct FileEditPlanner;

impl FileEditPlanner {
    pub fn new() -> Self { Self }

    pub fn plan(&self, file_sets: &[FileSet]) -> anyhow::Result<Vec<PlannedEdit>> {
        let mut edits = Vec::new();
        for fs in file_sets {
            for op in &fs.files {
                let content = self.apply_edits(op)?;
                edits.push(PlannedEdit {
                    file_path: op.file_path.clone(),
                    new_content: content,
                });
            }
        }
        Ok(edits)
    }

    fn apply_edits(&self, op: &FileOperation) -> anyhow::Result<String> {
        let mut lines: Vec<String> = Vec::new();
        // If file exists, read it; otherwise start empty
        if op.file_path.exists() {
            let content = std::fs::read_to_string(&op.file_path)
                .unwrap_or_default();
            lines = content.lines().map(String::from).collect();
        }

        for edit in &op.edits {
            match edit {
                FileEditOp::Insert { line, content } => {
                    let idx = (*line).min(lines.len());
                    lines.insert(idx, content.clone());
                }
                FileEditOp::Delete { start_line, end_line } => {
                    let start = (*start_line).min(lines.len());
                    let end = (*end_line).min(lines.len());
                    if start < end { lines.drain(start..end); }
                }
                FileEditOp::Replace { start_line, end_line, new_content } => {
                    let start = (*start_line).min(lines.len());
                    let end = (*end_line).min(lines.len());
                    if start < end { lines.drain(start..end); }
                    lines.insert(start, new_content.clone());
                }
                FileEditOp::Create { content } => {
                    lines = content.lines().map(String::from).collect();
                }
                FileEditOp::DeleteFile => {
                    lines.clear();
                }
            }
        }
        Ok(lines.join("\n"))
    }
}
