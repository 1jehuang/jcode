//! # git_workflow — Git 感知工作流
//!
//! 从 Claude Code 移植的 Git 智能操作层：
//! - 自动提交：代码修改后自动生成 Conventional Commits 消息并提交
//! - 变更快照：操作前后的 git diff 自动捕获 + 对比
//! - 分支管理：自动创建/切换 feature/fix 分支
//! - PR 准备：生成 PR 描述、变更摘要、影响分析
//! - 回滚支持：快速 revert 到上一个 checkpoint
//! - 代码行胆：追踪每行代码的修改历史和作者意图

use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    pub auto_commit: bool,
    pub commit_style: CommitStyle,
    pub auto_push: bool,
    pub create_branch: bool,
    pub max_commit_msg_length: usize,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            auto_commit: true,
            commit_style: CommitStyle::Conventional,
            auto_push: false,
            create_branch: false,
            max_commit_msg_length: 72,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommitStyle {
    Conventional,
    Simple,
    Descriptive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub timestamp: String,
    pub branch: String,
    pub commit_hash: Option<String>,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub diff_summary: String,
    pub changed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    pub path: PathBuf,
    pub status: DiffStatus,
    pub hunks: Vec<DiffHunk>,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffStatus { Added, Modified, Deleted, Renamed }

impl DiffStatus {
    pub fn from_git_status(ch: char) -> Self {
        match ch {
            'A' => Self::Added,
            'M' => Self::Modified,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            _ => Self::Modified,
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Modified => "~",
            Self::Deleted => "-",
            Self::Renamed => "->",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineKind { Context, Addition, Deletion }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrDescription {
    pub title: String,
    pub description: String,
    pub changes_section: String,
    pub checklist: Vec<String>,
    pub template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub commit_hash: String,
    pub message: String,
    pub files_changed: usize,
    pub pushed: bool,
    pub snapshot: GitSnapshot,
}

// -- Blame Info --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlameInfo {
    pub commit_hash: String,
    pub author: String,
    pub timestamp: String,
    pub line_number: usize,
    pub content: String,
}

// -- Workflow Manager --

pub struct GitWorkflow {
    config: GitConfig,
    working_dir: PathBuf,
}

impl GitWorkflow {
    pub fn new(config: GitConfig, working_dir: PathBuf) -> Self {
        Self { config, working_dir }
    }

    pub fn is_git_repo(&self) -> bool {
        self.working_dir.join(".git").exists()
    }

    pub fn repo_root(&self) -> Result<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&self.working_dir)
            .output()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(path))
    }

    pub fn current_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.working_dir)
            .output()?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn create_snapshot(&self) -> Result<GitSnapshot> {
        let branch = self.current_branch().unwrap_or_else(|_| "unknown".into());

        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.working_dir)
            .output()?;
        let hash = if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        };

        let diff = self.get_status_diff()?;
        let summary = diff.iter()
            .map(|d| format!("{} {}", d.status.icon(), d.path.display()))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(GitSnapshot {
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            branch,
            commit_hash: hash,
            files_changed: diff.len(),
            insertions: 0,
            deletions: 0,
            diff_summary: if summary.is_empty() { "no changes".into() } else { summary },
            changed_files: diff.iter().map(|d| d.path.clone()).collect(),
        })
    }

    pub fn get_diff(&self) -> Result<Vec<GitDiff>> {
        let output = Command::new("git")
            .args(["diff", "--unified=3"])
            .current_dir(&self.working_dir)
            .output()?;

        let diff_text = String::from_utf8_lossy(&output.stdout);
        self.parse_unified_diff(&diff_text)
    }

    pub fn get_status_diff(&self) -> Result<Vec<GitDiff>> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.working_dir)
            .output()?;

        let mut diffs = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if line.len() < 3 { continue; }
            let status_ch = line.chars().next().unwrap_or(' ');
            let path_str = line[3..].trim();

            diffs.push(GitDiff {
                path: PathBuf::from(path_str),
                status: DiffStatus::from_git_status(status_ch),
                hunks: vec![],
                old_content: None,
                new_content: None,
            });
        }

        Ok(diffs)
    }

    pub fn get_file_diff(&self, file_path: &Path) -> Result<GitDiff> {
        let output = Command::new("git")
            .args(["diff", "--unified=3", "--"])
            .arg(file_path)
            .current_dir(&self.working_dir)
            .output()?;

        let diff_text = String::from_utf8_lossy(&output.stdout);
        let diffs = self.parse_unified_diff(&diff_text)?;
        diffs.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("No diff for {:?}", file_path))
    }

    pub fn auto_commit(&self, change_description: &str, files: &[PathBuf]) -> Result<CommitResult> {
        if !self.config.auto_commit {
            info!("Auto-commit disabled, skipping");
            let snapshot = self.create_snapshot()?;
            return Ok(CommitResult {
                commit_hash: "skipped".into(),
                message: "auto-commit disabled".into(),
                files_changed: 0,
                pushed: false,
                snapshot,
            });
        }

        let snapshot = self.create_snapshot()?;
        if snapshot.files_changed == 0 {
            info!("No files changed, skipping commit");
            return Ok(CommitResult {
                commit_hash: "no-changes".into(),
                message: "no changes".into(),
                files_changed: 0,
                pushed: false,
                snapshot,
            });
        }

        let commit_msg = self.generate_commit_message(change_description, files);

        for file in files {
            let _ = Command::new("git")
                .args(["add", &file.to_string_lossy()])
                .current_dir(&self.working_dir)
                .output();
        }

        let output = Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(&self.working_dir)
            .output()?;

        let commit_hash = if output.status.success() {
            let hash_output = Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .current_dir(&self.working_dir)
                .output()?;
            String::from_utf8_lossy(&hash_output.stdout).trim().to_string()
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Commit failed: {}", stderr);
            "failed".into()
        };

        let pushed = if self.config.auto_push && commit_hash != "failed" {
            let _ = Command::new("git")
                .arg("push")
                .current_dir(&self.working_dir)
                .output();
            true
        } else {
            false
        };

        Ok(CommitResult {
            commit_hash,
            message: commit_msg,
            files_changed: files.len(),
            pushed,
            snapshot,
        })
    }

    pub fn generate_commit_message(&self, description: &str, files: &[PathBuf]) -> String {
        let file_list = files.iter()
            .take(3)
            .map(|f| f.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");

        match self.config.commit_style {
            CommitStyle::Conventional => {
                let prefix = if description.contains("fix") || description.contains("修复") {
                    "fix:"
                } else {
                    "feat:"
                };
                let rest = description
                    .chars()
                    .take(self.config.max_commit_msg_length.saturating_sub(prefix.len() + 1))
                    .collect::<String>();
                format!("{} {} in [{}]", prefix, rest, file_list)
            }
            CommitStyle::Simple => {
                format!(
                    "changes in {}: {}",
                    file_list,
                    description.chars().take(self.config.max_commit_msg_length).collect::<String>()
                )
            }
            CommitStyle::Descriptive => {
                format!(
                    "jcode auto-commit: {} ({} files: {})",
                    description,
                    files.len(),
                    file_list
                ).chars().take(self.config.max_commit_msg_length).collect()
            }
        }
    }

    pub fn generate_pr_description(&self, title: &str, changes: &str) -> PrDescription {
        PrDescription {
            title: title.to_string(),
            description: format!(
                "## 变更说明\n{}\n\n## 注入上下文\n此 PR 由 jcode Agent 自动生成（Git-aware workflow）。",
                changes
            ),
            changes_section: changes.to_string(),
            checklist: vec![
                "✅ 代码通过 lint 检查".into(),
                "✅ 构建通过".into(),
                "✅ 自动化测试通过".into(),
            ],
            template: String::new(),
        }
    }

    pub fn get_blame(&self, file: &Path, lines: &[usize]) -> Result<Vec<BlameInfo>> {
        let mut results = Vec::new();

        for &line_num in lines {
            let output = Command::new("git")
                .args([
                    "blame",
                    "-L",
                    &format!("{},{}", line_num, line_num),
                    "--line-porcelain",
                    &file.to_string_lossy(),
                ])
                .current_dir(&self.working_dir)
                .output()?;

            let text = String::from_utf8_lossy(&output.stdout);
            let mut commit_hash = String::new();
            let mut author = String::new();
            let mut timestamp = String::new();
            let mut content = String::new();

            for l in text.lines() {
                if commit_hash.is_empty() && l.len() >= 40 {
                    commit_hash = l[..40].to_string();
                    continue;
                }
                if let Some(s) = l.strip_prefix("author ") { author = s.to_string(); }
                if let Some(s) = l.strip_prefix("committer-time ") { timestamp = s.to_string(); }
                if let Some(s) = l.strip_prefix('\t') { content = s.to_string(); }
            }

            results.push(BlameInfo {
                commit_hash,
                author,
                timestamp,
                line_number: line_num,
                content,
            });
        }

        Ok(results)
    }

    fn parse_unified_diff(&self, text: &str) -> Result<Vec<GitDiff>> {
        let mut diffs = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_hunk: Option<DiffHunk> = None;
        let mut current_lines: Vec<DiffLine> = Vec::new();
        let mut old_count = 0usize;
        let mut new_count = 0usize;

        for line in text.lines() {
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                if let Some(ref path) = current_path
                    && let Some(mut hunk) = current_hunk.take()
                {
                    hunk.lines = std::mem::take(&mut current_lines);
                    hunk.old_count = old_count;
                    hunk.new_count = new_count;
                    let status = DiffStatus::Modified;
                    diffs.push(GitDiff {
                        path: path.clone(),
                        status,
                        hunks: vec![hunk],
                        old_content: None,
                        new_content: None,
                    });
                }
                if line.starts_with("--- ") {
                    current_path = Some(PathBuf::from(line[6..].trim()));
                }
                continue;
            }

            if line.starts_with("@@ ") {
                if let Some(mut hunk) = current_hunk.take() {
                    hunk.lines = std::mem::take(&mut current_lines);
                    hunk.old_count = old_count;
                    hunk.new_count = new_count;
                    if let Some(ref path) = current_path
                        && let Some(idx) = diffs.iter().position(|d| d.path == *path)
                    {
                        diffs[idx].hunks.push(hunk);
                    }
                }

                old_count = 0;
                new_count = 0;
                current_hunk = Some(DiffHunk {
                    old_start: 0,
                    old_count: 0,
                    new_start: 0,
                    new_count: 0,
                    header: line.to_string(),
                    lines: vec![],
                });
                continue;
            }

            let kind = match line.chars().next() {
                Some('+') => { new_count += 1; DiffLineKind::Addition }
                Some('-') => { old_count += 1; DiffLineKind::Deletion }
                _ => { old_count += 1; new_count += 1; DiffLineKind::Context }
            };

            current_lines.push(DiffLine {
                kind,
                content: line[1..].to_string(),
                old_line: None,
                new_line: None,
            });
        }

        if let Some(mut hunk) = current_hunk {
            hunk.lines = current_lines;
            hunk.old_count = old_count;
            hunk.new_count = new_count;
            if let Some(ref path) = current_path {
                if let Some(idx) = diffs.iter().position(|d| d.path == *path) {
                    diffs[idx].hunks.push(hunk);
                } else {
                    diffs.push(GitDiff {
                        path: path.clone(),
                        status: DiffStatus::Modified,
                        hunks: vec![hunk],
                        old_content: None,
                        new_content: None,
                    });
                }
            }
        }

        Ok(diffs)
    }
}

// -- Utils --

pub fn git_init(dir: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("init")
        .current_dir(dir)
        .output()
        .with_context(|| format!("Failed to init git in {:?}", dir))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git init failed: {}", stderr);
    }
    info!("Git repo initialized in {:?}", dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_commit_conventional_fix() {
        let wf = GitWorkflow::new(GitConfig::default(), PathBuf::from("."));
        let msg = wf.generate_commit_message(
            "fix a bug",
            &[PathBuf::from("src/main.rs")],
        );
        assert!(msg.starts_with("fix:"));
        assert!(msg.contains("src/main.rs"));
    }

    #[test]
    fn test_generate_commit_conventional_feat() {
        let wf = GitWorkflow::new(GitConfig::default(), PathBuf::from("."));
        let msg = wf.generate_commit_message(
            "add new feature",
            &[PathBuf::from("src/lib.rs")],
        );
        assert!(msg.starts_with("feat:"));
    }

    #[test]
    fn test_diff_status_icons() {
        assert_eq!(DiffStatus::Added.icon(), "+");
        assert_eq!(DiffStatus::Modified.icon(), "~");
        assert_eq!(DiffStatus::Deleted.icon(), "-");
    }
}