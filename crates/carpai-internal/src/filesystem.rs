//! Virtual File System Trait - Unified file operations with security
//!
//! Abstracts file system access for agent tool execution:
//!
//! ## Design Goals
//!
//! 1. **Security**: Server mode restricts all operations to a workspace root.
//!    Path traversal attacks are prevented at the trait level.
//!
//! 2. **Audit**: Every write/delete operation produces an audit record.
//!
//! 3. **VFS Support**: Operations can be virtual (in-memory, git-backed,
//!    or a real filesystem).
//!
//! ## Implementations
//!
//! | Product | Implementation | Behavior |
//! |---------|---------------|----------|
//! | `carpai-cli` | `LocalFileSystem` | Direct `std::fs` / `tokio::fs`, mirrors existing `src/tool/write.rs` etc. |
//! | `carpai-server` | `WorkspaceFileSystem` | Chroot to tenant workspace + audit log + path sandboxing |
//! | `testing` | `InMemoryFileSystem` | HashMap-based, no I/O |

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;

// ========================================================================
// Core Trait
// ========================================================================

/// Virtual file system interface for agent operations
///
/// All paths are **relative** to the configured root. The implementation
/// is responsible for:
/// - Resolving relative → absolute paths
/// - Preventing path traversal (`../` escapes)
/// - Enforcing permissions (read-only vs read-write)
/// - Recording audit events for mutations
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    // --- Basic File Operations ---

    /// Read entire file content as UTF-8 string
    async fn read_file(&self, path: &Path) -> Result<String, FsError>;

    /// Read file as raw bytes (for binary files)
    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>, FsError>;

    /// Write content to a file (creates parent dirs if needed)
    ///
    /// Returns the number of bytes written and an audit record ID.
    async fn write_file(&self, path: &Path, content: &str) -> Result<FileWriteResult, FsError>;

    /// Write raw bytes to a file
    async fn write_file_bytes(&self, path: &Path, data: &[u8]) -> Result<FileWriteResult, FsError>;

    /// Delete a file
    async fn delete_file(&self, path: &Path) -> Result<(), FsError>;

    /// Check if a file exists
    async fn exists(&self, path: &Path) -> Result<bool, FsError>;

    /// Get file metadata (size, modified time, permissions)
    async fn metadata(&self, path: &Path) -> Result<FileMeta, FsError>;

    // --- Directory Operations ---

    /// List directory contents (non-recursive by default)
    async fn list_dir(
        &self,
        path: &Path,
        recursive: bool,
    ) -> Result<Vec<FileEntry>, FsError>;

    /// Create a directory (and parents if needed)
    async fn create_dir(&self, path: &Path) -> Result<(), FsError>;

    /// Delete a directory (must be empty unless `recursive = true`)
    async fn delete_dir(&self, path: &Path, recursive: bool) -> Result<(), FsError>;

    // --- Search ---

    /// Search files by name pattern (glob-style)
    async fn search_files(
        &self,
        pattern: &str,
        in_path: &Path,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, FsError>;

    /// Search file contents (grep-like)
    async fn search_content(
        &self,
        query: &str,
        in_path: &Path,
        options: SearchOptions,
    ) -> Result<Vec<ContentMatch>, FsError>;

    // --- Git Operations (optional extension) ---

    /// Get git diff for a path (if in a git repository)
    async fn git_diff(&self, path: &Path, staged: bool) -> Result<String, FsError>;

    /// Get git status for a path
    async fn git_status(&self, path: &Path) -> Result<String, FsError>;

    /// Get git blame for a file
    async fn git_blame(&self, path: &Path) -> Result<String, FsError>;

    // --- Watch (optional) ---

    /// Watch a path for changes (returns a stream of events)
    ///
    /// Not all implementations support watching. Returns `FsError::Unsupported`
    /// if not available.
    async fn watch(
        &self,
        path: &Path,
    ) -> Result<Pin<Box<dyn tokio_stream::Stream<Item = FsEvent> + Send>>, FsError>;

    // --- Admin / Security ---

    /// Resolve a relative path to its absolute form within this VFS
    ///
    /// This is used for validation — callers can check if a path would
    /// escape the root before calling other methods.
    fn resolve(&self, path: &Path) -> Result<PathBuf, FsError>;

    /// Get the root directory of this VFS
    fn root(&self) -> &Path;

    /// Check if a path is within the allowed scope
    fn is_allowed(&self, path: &Path) -> bool;
}

// ========================================================================
// Result Types
// ========================================================================

/// Result of a file write operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWriteResult {
    /// Bytes written
    pub bytes_written: u64,

    /// Whether this was a new file creation (vs overwrite)
    pub created: bool,

    /// Audit record ID
    pub audit_id: Option<String>,

    /// Previous content hash (for change detection)
    pub previous_hash: Option<String>,

    /// New content hash
    pub new_hash: String,
}

/// File metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    /// Absolute path
    pub path: PathBuf,

    /// File size in bytes
    pub size: u64,

    /// Is directory
    pub is_dir: bool,

    /// Is symlink
    pub is_symlink: bool,

    /// Last modification time
    pub modified_at: SystemTime,

    /// Creation time (if available)
    pub created_at: Option<SystemTime>,

    /// File extension (e.g., "rs", "ts")
    pub extension: Option<String>,

    /// Content hash (SHA-256 hex, computed on demand)
    pub content_hash: Option<String>,
}

/// A single entry from list_dir
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub meta: FileMeta,
}

// ========================================================================
// Search Types
// ========================================================================

/// Result from filename search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: PathBuf,
    pub meta: FileMeta,
    /// Relevance score (0.0 - 1.0, higher = better match)
    pub score: f64,
}

/// Options for content search
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Case insensitive
    #[serde(default)]
    pub case_insensitive: bool,

    /// Use regex instead of plain text
    #[serde(default)]
    pub regex: bool,

    /// Max results per file
    #[serde(default)]
    pub max_matches_per_file: usize,

    /// Include context lines before each match
    #[serde(default)]
    pub context_lines_before: usize,

    /// Include context lines after each match
    #[serde(default)]
    pub context_lines_after: usize,

    /// File extensions to include (empty = all)
    #[serde(default)]
    pub extensions: Vec<String>,

    /// Patterns to exclude (glob-style)
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

/// A single content match (like grep output)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentMatch {
    /// File where match was found
    pub file: PathBuf,

    /// Line number (1-indexed)
    pub line_number: usize,

    /// The matching line content
    pub line: String,

    /// Byte offset of match start
    pub byte_offset: usize,

    /// Length of the match
    pub match_length: usize,

    /// Lines before match (context)
    pub before_context: Vec<String>,

    /// Lines after match (context)
    pub after_context: Vec<String>,
}

// ========================================================================
// File System Events (for watching)
// ========================================================================

/// Event emitted when watching files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsEvent {
    /// File was created
    Created { path: PathBuf },
    /// File was modified
    Modified { path: PathBuf },
    /// File was deleted
    Deleted { path: PathBuf },
    /// File was renamed
    Renamed { old_path: PathBuf, new_path: PathBuf },
    /// Error occurred while watching
    Error { path: PathBuf, error: String },
}

// ========================================================================
// Errors
// ========================================================================

/// File system error types
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("File not found: {0}")]
    NotFound(String),

    #[error("Path escape detected: {path} escapes root {root}")]
    PathEscape { path: String, root: String },

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Already exists: {0}")]
    AlreadyExists(String),

    #[error("Directory not empty: {0} (use recursive delete)")]
    NotEmpty(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Not a file: {0}")]
    NotAFile(String),

    #[error("Not a directory: {0}")]
    NotADirectory(String),

    #[error("Encoding error: {0}")]
    Encoding(String),

    #[error("Operation not supported")]
    Unsupported,

    #[error("Quota exceeded: limit={limit_mb}MB, current={current_mb}MB")]
    QuotaExceeded { limit_mb: u64, current_mb: u64 },

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_meta_serialization() {
        let meta = FileMeta {
            path: PathBuf::from("/tmp/test.rs"),
            size: 1024,
            is_dir: false,
            is_symlink: false,
            modified_at: SystemTime::now(),
            created_at: None,
            extension: Some("rs".into()),
            content_hash: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("test.rs"));
        assert!(json.contains("rs"));
    }

    #[test]
    fn test_fs_event_serialization() {
        let event = FsEvent::Created { path: PathBuf::from("/new.txt") };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Created"));
    }

    #[test]
    fn test_search_options_default() {
        let opts = SearchOptions::default();
        assert!(!opts.case_insensitive); // default false
        assert!(opts.extensions.is_empty());
    }

    #[test]
    fn test_content_match() {
        let m = ContentMatch {
            file: PathBuf::from("/main.rs"),
            line_number: 42,
            line: "fn main() {".into(),
            byte_offset: 1000,
            match_length: 9,
            before_context: vec![],
            after_context: vec!["}".into()],
        };
        assert_eq!(m.line_number, 42);
    }
}
