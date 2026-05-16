//! Git Workflow Commands for CarpAI
//!
//! Provides enhanced Git operations with AI assistance:
//! - Smart commit message generation
//! - Conflict resolution suggestions
//! - Branch management
//! - Diff analysis and visualization

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;

/// Git command configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    pub repo_path: PathBuf,
    pub auto_stage: bool,
    pub ai_generate_messages: bool,
    pub default_remote: Option<String>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            repo_path: PathBuf::from("."),
            auto_stage: false,
            ai_generate_messages: true,
            default_remote: Some("origin".to_string()),
        }
    }
}

/// Commit information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// Branch information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub commit_hash: String,
    pub ahead: usize,
    pub behind: usize,
}

/// Diff information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffInfo {
    pub file_path: PathBuf,
    pub status: FileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub changes: Vec<DiffChange>,
}

/// File change status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    TypeChanged,
}

/// Single diff change (hunk)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffChange {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

/// Line in diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

/// Result of git operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitResult<T> {
    pub success: bool,
    pub data: Option<T>,
    pub message: String,
    pub ai_suggestion: Option<String>,
}

/// Git workflow trait for extensibility
#[async_trait]
pub trait GitWorkflow: Send + Sync {
    /// Create a new commit with optional AI-generated message
    async fn commit(&self, message: Option<&str>, amend: bool) -> Result<GitResult<CommitInfo>>;
    
    /// Create a new branch
    async fn create_branch(&self, name: &str, base: Option<&str>) -> Result<GitResult<BranchInfo>>;
    
    /// List branches
    async fn list_branches(&self, remote: bool) -> Result<Vec<BranchInfo>>;
    
    /// Show diff for files or staging area
    async fn show_diff(&self, staged: bool, file: Option<&str>) -> Result<GitResult<Vec<DiffInfo>>>;
    
    /// Interactive rebase
    async fn interactive_rebase(&self, commits: usize) -> Result<GitResult<()>>;
    
    /// Cherry-pick commits
    async fn cherry_pick(&self, commits: &[&str]) -> Result<GitResult<()>>;
    
    /// Stash changes
    async fn stash(&self, message: Option<&str>) -> Result<GitResult<String>>;
    
    /// Pop stash
    async fn stash_pop(&self, index: usize) -> Result<GitResult<()>>;
    
    /// Get current branch name
    async fn current_branch(&self) -> Result<String>;
    
    /// Get repository status summary
    async fn status_summary(&self) -> Result<RepoStatus>;
}

/// Repository status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub branch: String,
    pub clean: bool,
    pub staged_files: usize,
    pub modified_files: usize,
    pub untracked_files: usize,
    pub ahead: usize,
    pub behind: usize,
}

/// Default Git workflow implementation
pub struct DefaultGitWorkflow {
    config: GitConfig,
}

impl DefaultGitWorkflow {
    /// Create new Git workflow instance
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }

    /// Create with default config in current directory
    pub fn current_dir() -> Self {
        Self::new(GitConfig::default())
    }

    /// Execute git command and return output
    async fn exec_git(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.config.repo_path)
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git command failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Generate AI-powered commit message from diff
    async fn generate_commit_message(&self, staged: bool) -> Result<String> {
        let diff_output = self.exec_git(&if staged {
            vec!["diff", "--cached", "--stat"]
        } else {
            vec!["diff", "--stat"]
        }).await?;

        // Simple heuristic-based message generation (can be replaced with LLM later)
        let lines: Vec<&str> = diff_output.lines().collect();
        let file_count = lines.len().saturating_sub(1); // Exclude summary line
        
        if file_count == 0 {
            return Ok("chore: empty commit".to_string());
        }

        // Analyze changed files to infer type
        let mut has_code_changes = false;
        let has_doc_changes = false;
        let has_test_changes = false;

        for line in &lines {
            if line.ends_with(".rs") || line.ends_with(".ts") || line.ends_with(".py") {
                has_code_changes = true;
            } else if line.ends_with(".md") || line.ends_with(".txt") || line.ends_with(".rst") {
                has_doc_changes = true;
            } else if line.contains("test") || line.contains("spec") {
                has_test_changes = true;
            }
        }

        let prefix = if has_test_changes {
            "test"
        } else if has_doc_changes {
            "docs"
        } else if has_code_changes {
            "feat"
        } else {
            "chore"
        };

        Ok(format!("{}: update {} file(s)", prefix, file_count))
    }
}

#[async_trait]
impl GitWorkflow for DefaultGitWorkflow {
    async fn commit(&self, message: Option<&str>, amend: bool) -> Result<GitResult<CommitInfo>> {
        let msg = match message {
            Some(m) => m.to_string(),
            None => self.generate_commit_message(self.config.auto_stage).await?,
        };

        let mut args = vec!["commit"];
        
        if self.config.auto_stage && message.is_none() {
            args.push("-a");
        }
        
        if amend {
            args.push("--amend");
        }
        
        args.push("-m");
        args.push(&msg);

        let output = self.exec_git(&args).await?;
        
        // Get commit info
        let log_output = self.exec_git(&["log", "-1", "--format=%H|%s|%an|%ci"]).await?;
        let parts: Vec<&str> = log_output.split('|').collect();

        let commit_info = CommitInfo {
            hash: parts.get(0).unwrap_or(&"").to_string(),
            message: msg.clone(),
            author: parts.get(2).unwrap_or(&"unknown").to_string(),
            timestamp: chrono::Utc::now(),
            files_changed: 0,
            insertions: 0,
            deletions: 0,
        };

        Ok(GitResult {
            success: true,
            data: Some(commit_info),
            message: format!("Committed successfully: {}", msg),
            ai_suggestion: None,
        })
    }

    async fn create_branch(&self, name: &str, base: Option<&str>) -> Result<GitResult<BranchInfo>> {
        let mut args = vec!["checkout", "-b", name];
        
        if let Some(base_branch) = base {
            args.push(base_branch);
        }

        self.exec_git(&args).await?;

        let current = self.current_branch().await?;
        
        Ok(GitResult {
            success: true,
            data: Some(BranchInfo {
                name: name.to_string(),
                is_current: true,
                is_remote: false,
                commit_hash: String::new(),
                ahead: 0,
                behind: 0,
            }),
            message: format!("Created and switched to branch '{}'", name),
            ai_suggestion: Some("Consider pushing this branch to remote with `git push -u origin <branch>`".to_string()),
        })
    }

    async fn list_branches(&self, remote: bool) -> Result<Vec<BranchInfo>> {
        let mut args = vec!["branch", "-vv"];
        if remote {
            args.push("-r");
        }

        let output = self.exec_git(&args).await?;
        let current_branch = self.current_branch().await.ok();

        let branches: Vec<BranchInfo> = output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let is_current = line.starts_with('*');
                let name = line.trim_start_matches('*').trim().split_whitespace().next().unwrap_or("").to_string();
                
                BranchInfo {
                    name,
                    is_current,
                    is_remote: remote,
                    commit_hash: String::new(),
                    ahead: 0,
                    behind: 0,
                }
            })
            .collect();

        Ok(branches)
    }

    async fn show_diff(&self, staged: bool, file: Option<&str>) -> Result<GitResult<Vec<DiffInfo>>> {
        let mut args = vec!["diff"];
        if staged {
            args.push("--staged");
        }
        if let Some(f) = file {
            args.push("--");
            args.push(f);
        }

        let output = self.exec_git(&args).await?;
        
        // Parse diff output into structured data
        let diffs = parse_diff_output(&output);

        Ok(GitResult {
            success: true,
            data: Some(diffs),
            message: format!("Found {} changed file(s)", diffs.len()),
            ai_suggestion: if diffs.is_empty() {
                None
            } else {
                Some("Review changes carefully before committing".to_string())
            },
        })
    }

    async fn interactive_rebase(&self, _commits: usize) -> Result<GitResult<()>> {
        // Interactive rebase requires terminal interaction
        // This is a placeholder implementation
        Ok(GitResult {
            success: false,
            data: None,
            message: "Interactive rebase not supported in non-interactive mode".to_string(),
            ai_suggestion: Some("Use `git rebase -i` directly in your terminal".to_string()),
        })
    }

    async fn cherry_pick(&self, commits: &[&str]) -> Result<GitResult<()>> {
        let mut args = vec!["cherry-pick"];
        args.extend(commits);

        match self.exec_git(&args).await {
            Ok(_) => Ok(GitResult {
                success: true,
                data: None,
                message: format!("Cherry-picked {} commit(s)", commits.len()),
                ai_suggestion: None,
            }),
            Err(e) => Ok(GitResult {
                success: false,
                data: None,
                message: format!("Cherry-pick failed: {}", e),
                ai_suggestion: Some("Resolve conflicts manually or use --abort to cancel".to_string()),
            }),
        }
    }

    async fn stash(&self, message: Option<&str>) -> Result<GitResult<String>> {
        let mut args = vec!["stash", "push", "-m"];
        let msg = message.unwrap_or("WIP: auto-stash");
        args.push(msg);

        self.exec_git(&args).await?;

        Ok(GitResult {
            success: true,
            data: Some(format!("stash@{{0}}")),
            message: "Changes stashed successfully".to_string(),
            ai_suggestion: Some("Use `carpai git stash-pop` to restore changes".to_string()),
        })
    }

    async fn stash_pop(&self, index: usize) -> Result<GitResult<()>> {
        let stash_ref = format!("stash@{{{}}}", index);
        let args = ["stash", "pop", &stash_ref];

        match self.exec_git(&args).await {
            Ok(_) => Ok(GitResult {
                success: true,
                data: None,
                message: format!("Restored stash {}", index),
                ai_suggestion: None,
            }),
            Err(e) => Ok(GitResult {
                success: false,
                data: None,
                message: format!("Failed to pop stash: {}", e),
                ai_suggestion: Some("Check for conflicts or try a different stash index".to_string()),
            }),
        }
    }

    async fn current_branch(&self) -> Result<String> {
        let output = self.exec_git(&["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        Ok(output.trim().to_string())
    }

    async fn status_summary(&self) -> Result<RepoStatus> {
        let branch = self.current_branch().await.unwrap_or_else(|_| "unknown".to_string());
        let output = self.exec_git(&["status", "--porcelain"]).await?;

        let mut staged = 0;
        let modified = 0;
        let untracked = 0;

        for line in output.lines() {
            if line.is_empty() {
                continue;
            }
            
            let first_char = line.chars().next().unwrap_or(' ');
            let second_char = line.chars().nth(1).unwrap_or(' ');

            match (first_char, second_char) {
                ('?', _) => untracked += 1,
                (' ', '?') => untracked += 1,
                _ if first_char != ' ' => staged += 1,
                _ if second_char != ' ' && first_char == ' ' => modified += 1,
                _ => {}
            }
        }

        let clean = staged == 0 && modified == 0 && untracked == 0;

        Ok(RepoStatus {
            branch,
            clean,
            staged_files: staged,
            modified_files: modified,
            untracked_files: untracked,
            ahead: 0,
            behind: 0,
        })
    }
}

/// Parse git diff output into structured DiffInfo
fn parse_diff_output(diff_output: &str) -> Vec<DiffInfo> {
    let mut diffs = Vec::new();
    let mut current_file: Option<PathBuf> = None;
    let mut current_status = FileStatus::Modified;
    
    for line in diff_output.lines() {
        if line.starts_with("diff --git") {
            // Extract file path
            if let Some(file) = line.split('').last() {
                let cleaned = file.trim_start_matches('a/').trim_start_matches('b/');
                current_file = Some(PathBuf::from(cleaned));
            }
        } else if line.starts_with("new file mode") {
            current_status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            current_status = FileStatus::Deleted;
        } else if let Some(ref file) = current_file {
            if !diffs.iter().any(|d| d.file_path == *file) {
                diffs.push(DiffInfo {
                    file_path: file.clone(),
                    status: current_status.clone(),
                    additions: 0,
                    deletions: 0,
                    changes: Vec::new(),
                });
            }
        }
    }

    diffs
}

/// CLI command handler for git operations
pub struct GitCommands {
    workflow: Box<dyn GitWorkflow>,
}

impl GitCommands {
    /// Create new git commands instance
    pub fn new(workflow: Box<dyn GitWorkflow>) -> Self {
        Self { workflow }
    }

    /// Create with default workflow
    pub fn with_default_workflow() -> Self {
        Self::new(Box::new(DefaultGitWorkflow::current_dir()))
    }

    /// Handle commit command
    pub async fn handle_commit(&self, message: Option<&str>, amend: bool) -> Result<()> {
        let result = self.workflow.commit(message, amend).await?;
        
        println!("{}", result.message);
        if let Some(suggestion) = result.ai_suggestion {
            println!("💡 AI Suggestion: {}", suggestion);
        }
        
        Ok(())
    }

    /// Handle branch creation
    pub async fn handle_create_branch(&self, name: &str, base: Option<&str>) -> Result<()> {
        let result = self.workflow.create_branch(name, base).await?;
        
        println!("{}", result.message);
        if let Some(suggestion) = result.ai_suggestion {
            println!("💡 {}", suggestion);
        }
        
        Ok(())
    }

    /// Handle show diff
    pub async fn handle_diff(&self, staged: bool, file: Option<&str>) -> Result<()> {
        let result = self.workflow.show_diff(staged, file).await?;
        
        println!("{}", result.message);
        if let Some(diffs) = &result.data {
            for diff in diffs {
                let icon = match diff.status {
                    FileStatus::Added => "+",
                    FileStatus::Deleted => "-",
                    FileStatus::Modified => "~",
                    FileStatus::Renamed => "->",
                    _ => "?",
                };
                println!("  {} {} (+{} / -{})", icon, diff.file_path.display(), diff.additions, diff.deletions);
            }
        }
        if let Some(suggestion) = result.ai_suggestion {
            println!("💡 {}", suggestion);
        }
        
        Ok(())
    }

    /// Handle status display
    pub async fn handle_status(&self) -> Result<()> {
        let status = self.workflow.status_summary().await?;
        
        println!("On branch: {}", status.branch);
        if status.clean {
            println!("✅ Working tree clean");
        } else {
            if status.staged_files > 0 {
                println!("📦 Changes to be committed: {}", status.staged_files);
            }
            if status.modified_files > 0 {
                println!("✏️  Modified but not updated: {}", status.modified_files);
            }
            if status.untracked_files > 0 {
                println!("❓ Untracked files: {}", status.untracked_files);
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_config_default() {
        let config = GitConfig::default();
        assert_eq!(config.auto_stage, false);
        assert!(config.ai_generate_messages);
        assert_eq!(config.default_remote.as_deref(), Some("origin"));
    }

    #[test]
    fn test_parse_simple_diff() {
        let diff_output = "diff --git a/src/main.rs b/src/main.rs\nindex 1234567..abcdefg 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n";
        let diffs = parse_diff_output(diff_output);
        
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(diffs[0].status, FileStatus::Modified);
    }

    #[tokio::test]
    async fn test_default_workflow_creation() {
        let workflow = DefaultGitWorkflow::current_dir();
        assert_eq!(workflow.config.repo_path, PathBuf::from("."));
    }

    #[test]
    fn test_file_status_display() {
        assert_eq!(format!("{:?}", FileStatus::Added), "Added");
        assert_eq!(format!("{:?}", FileStatus::Deleted), "Deleted");
        assert_eq!(format!("{:?}", FileStatus::Modified), "Modified");
    }
}
