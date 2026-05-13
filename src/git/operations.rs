use std::path::PathBuf;

/// Information about a git branch
#[derive(Debug, Clone)]
pub struct GitBranchInfo {
    pub name: String,
    pub current: bool,
    pub commit_hash: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub last_commit_message: String,
    pub last_commit_author: String,
    pub last_commit_date: String,
}

/// A file change in the working tree
#[derive(Debug, Clone)]
pub struct GitFileChange {
    pub path: String,
    pub change_type: ChangeType,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeType {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

/// Full git context for LLM prompts
#[derive(Debug, Clone)]
pub struct GitContext {
    pub current_branch: String,
    pub repository_root: PathBuf,
    pub recent_commits: Vec<String>,
    pub staged_changes: Vec<GitFileChange>,
    pub unstaged_changes: Vec<GitFileChange>,
    pub untracked_files: Vec<String>,
    pub branches: Vec<GitBranchInfo>,
    pub remotes: Vec<String>,
    pub last_commit: String,
    pub ahead_behind: Option<(usize, usize)>,
}

/// Git operations wrapper
pub struct GitOperations {
    pub repo_path: PathBuf,
}

impl GitOperations {
    pub fn new(path: PathBuf) -> Self {
        GitOperations { repo_path: path }
    }

    pub fn current_branch(&self) -> Option<String> {
        self.git_exec(&["rev-parse", "--abbrev-ref", "HEAD"])
    }

    pub fn list_branches(&self) -> Vec<GitBranchInfo> {
        let output = self.git_exec_output(&["branch", "-vv"]);
        output.map(|out| {
            out.lines().filter_map(|line| {
                let line = line.trim();
                if line.is_empty() { return None; }
                let current = line.starts_with('*');
                let name = line.trim_start_matches("* ").split_whitespace().next()?.to_string();
                Some(GitBranchInfo {
                    name,
                    current,
                    commit_hash: String::new(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                    last_commit_message: String::new(),
                    last_commit_author: String::new(),
                    last_commit_date: String::new(),
                })
            }).collect()
        }).unwrap_or_default()
    }

    pub fn diff_staged(&self) -> Vec<GitFileChange> {
        self.parse_diff_output(&["diff", "--cached", "--stat"])
    }

    pub fn diff_unstaged(&self) -> Vec<GitFileChange> {
        self.parse_diff_output(&["diff", "--stat"])
    }

    pub fn untracked_files(&self) -> Vec<String> {
        self.git_exec_output(&["ls-files", "--others", "--exclude-standard"])
            .map(|out| out.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn recent_commits(&self, count: usize) -> Vec<String> {
        self.git_exec_output(&["log", &format!("-{}", count), "--oneline"])
            .map(|out| out.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn last_commit_message(&self) -> String {
        self.git_exec_output(&["log", "-1", "--format=%s"])
            .unwrap_or_default()
    }

    pub fn get_context(&self) -> GitContext {
        GitContext {
            current_branch: self.current_branch().unwrap_or_default(),
            repository_root: self.repo_path.clone(),
            recent_commits: self.recent_commits(10),
            staged_changes: self.diff_staged(),
            unstaged_changes: self.diff_unstaged(),
            untracked_files: self.untracked_files(),
            branches: self.list_branches(),
            remotes: self.list_remotes(),
            last_commit: self.last_commit_message(),
            ahead_behind: None,
        }
    }

    pub fn create_branch(&self, name: &str) -> Result<String, String> {
        self.git_exec_result(&["checkout", "-b", name])
            .map(|_| format!("Created and switched to branch '{}'", name))
    }

    pub fn checkout_branch(&self, name: &str) -> Result<String, String> {
        self.git_exec_result(&["checkout", name])
            .map(|_| format!("Switched to branch '{}'", name))
    }

    pub fn delete_branch(&self, name: &str, force: bool) -> Result<String, String> {
        let mut args = vec!["branch"];
        if force { args.push("-D"); } else { args.push("-d"); }
        args.push(name);
        self.git_exec_result(&args)
            .map(|_| format!("Deleted branch '{}'", name))
    }

    pub fn format_diff(&self, staged: bool) -> String {
        let args = if staged {
            &["diff", "--cached", "--no-color"] as &[&str]
        } else {
            &["diff", "--no-color"]
        };
        self.git_exec_output(args).unwrap_or_default()
    }

    pub fn format_context_summary(&self) -> String {
        let ctx = self.get_context();
        let mut output = String::new();
        output.push_str(&format!("Branch: {}\n", ctx.current_branch));
        output.push_str(&format!("Root: {:?}\n", ctx.repository_root));
        output.push_str(&format!("Staged: {} files\n", ctx.staged_changes.len()));
        output.push_str(&format!("Unstaged: {} files\n", ctx.unstaged_changes.len()));
        output.push_str(&format!("Untracked: {} files\n", ctx.untracked_files.len()));
        output.push_str(&format!("Recent commits:\n"));
        for commit in &ctx.recent_commits {
            output.push_str(&format!("  {}\n", commit));
        }
        output
    }

    fn list_remotes(&self) -> Vec<String> {
        self.git_exec_output(&["remote"])
            .map(|out| out.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    }

    fn parse_diff_output(&self, args: &[&str]) -> Vec<GitFileChange> {
        self.git_exec_output(args)
            .map(|out| {
                out.lines().filter_map(|line| {
                    if line.is_empty() { return None; }
                    let parts: Vec<&str> = line.split('|').collect();
                    let path = parts.first()?.trim().to_string();
                    let stats = parts.get(1).map(|s| s.trim()).unwrap_or("");
                    let additions = stats.matches('+').count();
                    let deletions = stats.matches('-').count();
                    Some(GitFileChange {
                        path,
                        change_type: ChangeType::Modified,
                        additions,
                        deletions,
                    })
                }).collect()
            })
            .unwrap_or_default()
    }

    fn git_exec(&self, args: &[&str]) -> Option<String> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo_path)
            .args(args)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
    }

    fn git_exec_output(&self, args: &[&str]) -> Option<String> {
        self.git_exec(args)
    }

    fn git_exec_result(&self, args: &[&str]) -> Result<String, String> {
        self.git_exec(args).ok_or_else(|| {
            format!("Git command failed: {}", args.join(" "))
        })
    }
}