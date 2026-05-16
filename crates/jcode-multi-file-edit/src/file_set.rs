use std::path::PathBuf;

/// A set of files to be edited atomically.
#[derive(Debug, Clone)]
pub struct FileSet {
    pub files: Vec<FileOperation>,
    pub description: String,
}

/// A single file operation within an atomic set.
#[derive(Debug, Clone)]
pub struct FileOperation {
    pub file_path: PathBuf,
    pub edits: Vec<FileEditOp>,
}

/// A discrete edit operation on a file.
#[derive(Debug, Clone)]
pub enum FileEditOp {
    Insert { line: usize, content: String },
    Delete { start_line: usize, end_line: usize },
    Replace { start_line: usize, end_line: usize, new_content: String },
    Create { content: String },
    DeleteFile,
}

impl FileSet {
    pub fn new(files: Vec<FileOperation>, description: &str) -> Self {
        Self { files, description: description.to_string() }
    }

    pub fn file_count(&self) -> usize { self.files.len() }
}
