#![cfg_attr(test, allow(dead_code))]

use crate::desktop_log;
use crate::single_session::{GitHubIssueBrowserState, GitHubIssuePreview, GitHubIssueVisualState};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const ISSUE_CACHE_SCHEMA_VERSION: u32 = 1;
const DEFAULT_ISSUE_SYNC_LIMIT: usize = 100;
const DEFAULT_COMMENT_THREAD_SYNC_LIMIT: usize = 40;

#[derive(Clone, Debug)]
pub(crate) struct GitHubIssueSyncSummary {
    pub(crate) repo: String,
    pub(crate) issue_count: usize,
    pub(crate) fetched_comment_threads: usize,
    pub(crate) comment_fetch_errors: usize,
    pub(crate) cache_path: PathBuf,
    pub(crate) elapsed: Duration,
    pub(crate) browser: GitHubIssueBrowserState,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct GitHubIssueCache {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    pub(crate) repo: String,
    #[serde(default)]
    pub(crate) synced_at: Option<String>,
    #[serde(default)]
    pub(crate) issues: Vec<CachedGitHubIssue>,
    #[serde(default)]
    pub(crate) local_overrides: Vec<CachedGitHubIssueOverride>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CachedGitHubIssue {
    pub(crate) number: u64,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
    #[serde(default)]
    pub(crate) labels: Vec<CachedGitHubLabel>,
    #[serde(default)]
    pub(crate) comment_count: Option<u32>,
    #[serde(default)]
    pub(crate) comments: Vec<CachedGitHubComment>,
    #[serde(default)]
    pub(crate) state: Option<String>,
    #[serde(default)]
    pub(crate) created_at: Option<String>,
    #[serde(default)]
    pub(crate) updated_at: Option<String>,
    #[serde(default)]
    pub(crate) assignees: Vec<String>,
    #[serde(default)]
    pub(crate) milestone: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CachedGitHubLabel {
    pub(crate) name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CachedGitHubComment {
    #[serde(default)]
    pub(crate) author: Option<String>,
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) created_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CachedGitHubIssueOverride {
    pub(crate) number: u64,
    #[serde(default)]
    pub(crate) priority: Option<String>,
    #[serde(default)]
    pub(crate) pinned: bool,
}

fn default_schema_version() -> u32 {
    ISSUE_CACHE_SCHEMA_VERSION
}

pub(crate) fn load_current_repo_issue_browser() -> Result<Option<GitHubIssueBrowserState>> {
    let Some(repo) = detect_current_github_repo()? else {
        return Ok(None);
    };
    match load_issue_browser_for_repo(&repo) {
        Ok(browser) => Ok(Some(browser)),
        Err(error) if is_missing_cache_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_missing_cache_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

pub(crate) fn load_issue_browser_for_repo(repo: &str) -> Result<GitHubIssueBrowserState> {
    Ok(issue_browser_from_cache(load_issue_cache_for_repo_raw(
        repo,
    )?))
}

#[allow(dead_code)]
pub(crate) fn write_issue_cache(cache: &GitHubIssueCache) -> Result<PathBuf> {
    write_issue_cache_to_root(cache, &issue_cache_root())
}

fn load_issue_cache_for_repo_raw(repo: &str) -> Result<GitHubIssueCache> {
    load_issue_cache_for_repo_raw_from_root(repo, &issue_cache_root())
}

fn load_issue_cache_for_repo_raw_from_root(repo: &str, root: &Path) -> Result<GitHubIssueCache> {
    let path = issue_cache_path_for_repo_in_root(root, repo);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read GitHub issue cache {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse GitHub issue cache {}", path.display()))
}

fn write_issue_cache_to_root(cache: &GitHubIssueCache, root: &Path) -> Result<PathBuf> {
    let path = issue_cache_path_for_repo_in_root(root, &cache.repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create GitHub issue cache dir {}",
                parent.display()
            )
        })?;
    }
    let raw =
        serde_json::to_string_pretty(cache).context("failed to serialize GitHub issue cache")?;
    std::fs::write(&path, raw)
        .with_context(|| format!("failed to write GitHub issue cache {}", path.display()))?;
    Ok(path)
}

pub(crate) fn sync_current_repo_issue_cache() -> Result<GitHubIssueSyncSummary> {
    let repo = detect_current_github_repo()?
        .context("could not find a GitHub origin remote for this repository")?;
    sync_issue_cache_for_repo(&repo)
}

pub(crate) fn sync_issue_cache_for_repo(repo: &str) -> Result<GitHubIssueSyncSummary> {
    let runner = SystemGitHubIssueCommandRunner;
    sync_issue_cache_for_repo_with_runner_and_root(
        repo,
        &runner,
        GitHubIssueSyncOptions::default(),
        &issue_cache_root(),
    )
}

#[derive(Clone, Copy, Debug)]
struct GitHubIssueSyncOptions {
    issue_limit: usize,
    comment_thread_limit: usize,
}

impl Default for GitHubIssueSyncOptions {
    fn default() -> Self {
        Self {
            issue_limit: DEFAULT_ISSUE_SYNC_LIMIT,
            comment_thread_limit: DEFAULT_COMMENT_THREAD_SYNC_LIMIT,
        }
    }
}

trait GitHubIssueCommandRunner {
    fn run_gh(&self, args: &[String]) -> Result<String>;
}

struct SystemGitHubIssueCommandRunner;

impl GitHubIssueCommandRunner for SystemGitHubIssueCommandRunner {
    fn run_gh(&self, args: &[String]) -> Result<String> {
        let output = Command::new("gh").args(args).output();
        let output = match output {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("GitHub CLI `gh` is not installed or not on PATH")
            }
            Err(error) => return Err(error).context("failed to run GitHub CLI `gh`"),
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            anyhow::bail!(
                "gh {} failed{}",
                args.join(" "),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }
        String::from_utf8(output.stdout).context("GitHub CLI emitted non-UTF-8 output")
    }
}

fn sync_issue_cache_for_repo_with_runner_and_root(
    repo: &str,
    runner: &dyn GitHubIssueCommandRunner,
    options: GitHubIssueSyncOptions,
    cache_root: &Path,
) -> Result<GitHubIssueSyncSummary> {
    let started = Instant::now();
    let existing_cache = load_issue_cache_for_repo_raw_from_root(repo, cache_root).ok();
    let existing_comments_by_number = existing_cache
        .as_ref()
        .map(existing_comments_by_number)
        .unwrap_or_default();
    let local_overrides = existing_cache
        .map(|cache| cache.local_overrides)
        .unwrap_or_default();

    let raw_issues = runner.run_gh(&issue_list_args(repo, options.issue_limit))?;
    let gh_issues: Vec<GhIssueListItem> = serde_json::from_str(&raw_issues)
        .with_context(|| format!("failed to parse gh issue list JSON for {repo}"))?;

    let mut issues = Vec::with_capacity(gh_issues.len());
    let mut fetched_comment_threads = 0usize;
    let mut comment_fetch_errors = 0usize;
    for (index, issue) in gh_issues.into_iter().enumerate() {
        let number = issue.number;
        let existing_comments = existing_comments_by_number
            .get(&number)
            .cloned()
            .unwrap_or_default();
        let should_fetch_comments =
            issue.comments.unwrap_or_default() > 0 && index < options.comment_thread_limit;
        let comments = if should_fetch_comments {
            match fetch_issue_comments(repo, number, runner) {
                Ok(comments) => {
                    fetched_comment_threads += 1;
                    comments
                }
                Err(error) => {
                    comment_fetch_errors += 1;
                    desktop_log::warn(format_args!(
                        "jcode-desktop: failed to refresh comments for GitHub issue {repo}#{number}: {error:#}"
                    ));
                    existing_comments
                }
            }
        } else {
            existing_comments
        };
        issues.push(issue.into_cached(comments));
    }

    let cache = GitHubIssueCache {
        schema_version: ISSUE_CACHE_SCHEMA_VERSION,
        repo: repo.to_string(),
        synced_at: Some(current_sync_label()),
        issues,
        local_overrides,
    };
    let cache_path = write_issue_cache_to_root(&cache, cache_root)?;
    let issue_count = cache.issues.len();
    let browser = issue_browser_from_cache(cache);
    Ok(GitHubIssueSyncSummary {
        repo: repo.to_string(),
        issue_count,
        fetched_comment_threads,
        comment_fetch_errors,
        cache_path,
        elapsed: started.elapsed(),
        browser,
    })
}

fn issue_list_args(repo: &str, limit: usize) -> Vec<String> {
    vec![
        "issue".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--state".to_string(),
        "open".to_string(),
        "--limit".to_string(),
        limit.to_string(),
        "--json".to_string(),
        "number,title,body,labels,comments,createdAt,updatedAt,assignees,milestone,state"
            .to_string(),
    ]
}

fn issue_comments_args(repo: &str, number: u64) -> Vec<String> {
    vec![
        "issue".to_string(),
        "view".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--json".to_string(),
        "comments".to_string(),
    ]
}

fn fetch_issue_comments(
    repo: &str,
    number: u64,
    runner: &dyn GitHubIssueCommandRunner,
) -> Result<Vec<CachedGitHubComment>> {
    let raw_comments = runner.run_gh(&issue_comments_args(repo, number))?;
    let view: GhIssueCommentsView = serde_json::from_str(&raw_comments)
        .with_context(|| format!("failed to parse gh issue view JSON for {repo}#{number}"))?;
    Ok(view
        .comments
        .into_iter()
        .map(GhIssueComment::into_cached)
        .collect())
}

fn existing_comments_by_number(cache: &GitHubIssueCache) -> HashMap<u64, Vec<CachedGitHubComment>> {
    cache
        .issues
        .iter()
        .map(|issue| (issue.number, issue.comments.clone()))
        .collect()
}

fn current_sync_label() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("unix:{seconds}")
}

#[derive(Debug, Deserialize)]
struct GhIssueListItem {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    labels: Vec<GhIssueLabel>,
    #[serde(default)]
    comments: Option<u32>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(default)]
    assignees: Vec<GhIssueUser>,
    #[serde(default)]
    milestone: Option<GhIssueMilestone>,
}

impl GhIssueListItem {
    fn into_cached(self, comments: Vec<CachedGitHubComment>) -> CachedGitHubIssue {
        CachedGitHubIssue {
            number: self.number,
            title: self.title,
            body: self.body,
            labels: self
                .labels
                .into_iter()
                .map(|label| CachedGitHubLabel { name: label.name })
                .collect(),
            comment_count: self.comments,
            comments,
            state: self.state,
            created_at: self.created_at,
            updated_at: self.updated_at,
            assignees: self
                .assignees
                .into_iter()
                .map(|assignee| assignee.login)
                .filter(|login| !login.is_empty())
                .collect(),
            milestone: self.milestone.map(|milestone| milestone.title),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhIssueLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhIssueUser {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhIssueMilestone {
    title: String,
}

#[derive(Debug, Deserialize)]
struct GhIssueCommentsView {
    #[serde(default)]
    comments: Vec<GhIssueComment>,
}

#[derive(Debug, Deserialize)]
struct GhIssueComment {
    #[serde(default)]
    author: Option<GhIssueUser>,
    #[serde(default)]
    body: String,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

impl GhIssueComment {
    fn into_cached(self) -> CachedGitHubComment {
        CachedGitHubComment {
            author: self.author.map(|author| author.login),
            body: self.body,
            created_at: self.created_at,
        }
    }
}

#[allow(dead_code)]
pub(crate) fn issue_cache_path_for_repo(repo: &str) -> PathBuf {
    issue_cache_path_for_repo_in_root(&issue_cache_root(), repo)
}

fn issue_cache_path_for_repo_in_root(root: &Path, repo: &str) -> PathBuf {
    root.join(format!("{}.json", repo_cache_key(repo)))
}

pub(crate) fn issue_cache_root() -> PathBuf {
    if let Some(path) = std::env::var_os("JCODE_DESKTOP_ISSUE_CACHE_DIR") {
        return PathBuf::from(path);
    }
    jcode_data_dir().join("desktop/github/issues")
}

fn jcode_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("JCODE_HOME") {
        return PathBuf::from(path);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".jcode");
    }
    PathBuf::from(".jcode")
}

fn repo_cache_key(repo: &str) -> String {
    repo.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn detect_current_github_repo() -> Result<Option<String>> {
    detect_github_repo_from_dir(&std::env::current_dir().context("failed to get current dir")?)
}

pub(crate) fn detect_github_repo_from_dir(dir: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["remote", "get-url", "origin"])
        .output();
    let Ok(output) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let remote = String::from_utf8_lossy(&output.stdout);
    Ok(parse_github_repo_from_remote(remote.trim()))
}

pub(crate) fn parse_github_repo_from_remote(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches(".git");
    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return normalize_repo(rest);
    }
    if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        return normalize_repo(rest);
    }
    for prefix in ["https://github.com/", "http://github.com/"] {
        if let Some(rest) = remote.strip_prefix(prefix) {
            return normalize_repo(rest);
        }
    }
    None
}

fn normalize_repo(raw: &str) -> Option<String> {
    let mut parts = raw.split('/');
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

pub(crate) fn issue_browser_from_cache(cache: GitHubIssueCache) -> GitHubIssueBrowserState {
    let override_by_number = cache
        .local_overrides
        .iter()
        .map(|override_| (override_.number, override_))
        .collect::<HashMap<_, _>>();
    let mut ranked = cache
        .issues
        .into_iter()
        .filter(|issue| {
            issue
                .state
                .as_deref()
                .unwrap_or("open")
                .eq_ignore_ascii_case("open")
        })
        .map(|issue| {
            let override_ = override_by_number.get(&issue.number).copied();
            let priority = issue_priority(&issue, override_);
            let score = issue_priority_score(&issue, override_);
            let preview = cached_issue_to_preview(&cache.repo, issue, priority, score, override_);
            (
                priority_rank(&preview.priority),
                std::cmp::Reverse(score),
                std::cmp::Reverse(preview.number),
                preview,
            )
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(rank, score, number, _)| (*rank, *score, *number));
    let mut issues = ranked
        .into_iter()
        .map(|(_, _, _, preview)| preview)
        .collect::<Vec<_>>();
    if let Some(first) = issues.first_mut() {
        first.state = GitHubIssueVisualState::Selected;
    }
    let sync_label = cache
        .synced_at
        .unwrap_or_else(|| "unsynced cache".to_string());
    GitHubIssueBrowserState {
        repo: cache.repo,
        filter_label: format!("priority · open · cached {sync_label}"),
        selected: 0,
        list_scroll: 0,
        preview_scroll: 0,
        issues,
    }
}

fn cached_issue_to_preview(
    _repo: &str,
    issue: CachedGitHubIssue,
    priority: String,
    score: i32,
    override_: Option<&CachedGitHubIssueOverride>,
) -> GitHubIssuePreview {
    let comment_count = issue.comment_count.unwrap_or(issue.comments.len() as u32);
    let labels = issue
        .labels
        .into_iter()
        .map(|label| label.name)
        .collect::<Vec<_>>();
    let body_lines = split_preview_lines(issue.body.unwrap_or_default(), 10);
    let comment_lines = issue
        .comments
        .into_iter()
        .rev()
        .take(4)
        .map(|comment| match comment.author {
            Some(author) if !author.is_empty() => {
                format!("{author}: {}", compact_line(&comment.body))
            }
            _ => compact_line(&comment.body),
        })
        .collect::<Vec<_>>();
    let age = issue
        .updated_at
        .or(issue.created_at)
        .map(|value| format!("updated {value}"))
        .unwrap_or_else(|| "cached".to_string());
    let mut reason = issue_priority_reason(&priority, score, &labels);
    if override_.is_some_and(|override_| override_.priority.is_some()) {
        reason.push_str(" · local override");
    }
    GitHubIssuePreview {
        number: issue.number,
        priority,
        title: issue.title,
        labels,
        age,
        comments: comment_count,
        state: GitHubIssueVisualState::Idle,
        body_lines,
        comment_lines,
        priority_reason: reason,
    }
}

fn split_preview_lines(text: String, limit: usize) -> Vec<String> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(limit)
        .map(compact_line)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        vec!["No cached issue body yet. Refresh the issue cache to pull full context.".to_string()]
    } else {
        lines
    }
}

fn compact_line(line: &str) -> String {
    let mut compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() > 240 {
        compact.truncate(237);
        compact.push_str("...");
    }
    compact
}

fn issue_priority(
    issue: &CachedGitHubIssue,
    override_: Option<&CachedGitHubIssueOverride>,
) -> String {
    if let Some(priority) = override_.and_then(|override_| override_.priority.as_deref()) {
        if let Some(normalized) = normalize_priority(priority) {
            return normalized.to_string();
        }
    }
    let labels = issue_label_names(issue);
    if labels.iter().any(|label| {
        matches!(
            label.as_str(),
            "p0" | "priority:p0" | "priority:critical" | "critical" | "sev0"
        )
    }) {
        return "P0".to_string();
    }
    if labels.iter().any(|label| {
        matches!(
            label.as_str(),
            "p1" | "priority:p1" | "priority:high" | "high" | "sev1"
        )
    }) {
        return "P1".to_string();
    }
    if labels
        .iter()
        .any(|label| label.contains("regression") || label.contains("crash"))
        && labels.iter().any(|label| label.contains("bug"))
    {
        return "P1".to_string();
    }
    "P2".to_string()
}

fn normalize_priority(priority: &str) -> Option<&'static str> {
    match priority.trim().to_ascii_lowercase().as_str() {
        "p0" | "0" | "critical" => Some("P0"),
        "p1" | "1" | "high" => Some("P1"),
        "p2" | "2" | "medium" | "normal" | "low" => Some("P2"),
        _ => None,
    }
}

fn issue_priority_score(
    issue: &CachedGitHubIssue,
    override_: Option<&CachedGitHubIssueOverride>,
) -> i32 {
    let labels = issue_label_names(issue);
    let text = format!(
        "{} {}",
        issue.title,
        issue.body.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    let mut score = 0;
    if override_.is_some_and(|override_| override_.pinned) {
        score += 50;
    }
    for label in &labels {
        if label.contains("regression") {
            score += 18;
        }
        if label.contains("crash") || label.contains("panic") {
            score += 16;
        }
        if label.contains("bug") {
            score += 10;
        }
        if label.contains("desktop") {
            score += 4;
        }
    }
    for keyword in [
        "regression",
        "crash",
        "panic",
        "data loss",
        "hang",
        "broken",
    ] {
        if text.contains(keyword) {
            score += 5;
        }
    }
    score += (issue.comment_count.unwrap_or(issue.comments.len() as u32) as i32).min(10);
    if issue
        .milestone
        .as_deref()
        .is_some_and(|milestone| !milestone.is_empty())
    {
        score += 3;
    }
    if !issue.assignees.is_empty() {
        score += 2;
    }
    score
}

fn issue_label_names(issue: &CachedGitHubIssue) -> Vec<String> {
    issue
        .labels
        .iter()
        .map(|label| label.name.trim().to_ascii_lowercase())
        .collect()
}

fn priority_rank(priority: &str) -> u8 {
    match priority {
        "P0" => 0,
        "P1" => 1,
        _ => 2,
    }
}

fn issue_priority_reason(priority: &str, score: i32, labels: &[String]) -> String {
    let label_summary = if labels.is_empty() {
        "no labels".to_string()
    } else {
        labels.join(",")
    };
    format!("{priority} from labels/signals ({label_summary}), score {score}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug)]
    enum FakeGhResponse {
        Ok(String),
        Err(String),
    }

    struct FakeGhRunner {
        list_output: String,
        comment_outputs: HashMap<u64, FakeGhResponse>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl FakeGhRunner {
        fn new(list_output: String) -> Self {
            Self {
                list_output,
                comment_outputs: HashMap::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with_comment_output(mut self, issue_number: u64, output: String) -> Self {
            self.comment_outputs
                .insert(issue_number, FakeGhResponse::Ok(output));
            self
        }

        fn with_comment_error(mut self, issue_number: u64, error: &str) -> Self {
            self.comment_outputs
                .insert(issue_number, FakeGhResponse::Err(error.to_string()));
            self
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl GitHubIssueCommandRunner for FakeGhRunner {
        fn run_gh(&self, args: &[String]) -> Result<String> {
            self.calls.borrow_mut().push(args.to_vec());
            match args {
                [issue, list, ..] if issue == "issue" && list == "list" => {
                    Ok(self.list_output.clone())
                }
                [issue, view, number, ..] if issue == "issue" && view == "view" => {
                    let number = number.parse::<u64>().context("fake gh issue number")?;
                    match self.comment_outputs.get(&number) {
                        Some(FakeGhResponse::Ok(output)) => Ok(output.clone()),
                        Some(FakeGhResponse::Err(error)) => anyhow::bail!("{}", error),
                        None => anyhow::bail!("unexpected fake gh comment fetch for #{number}"),
                    }
                }
                _ => anyhow::bail!("unexpected fake gh args: {args:?}"),
            }
        }
    }

    fn unique_issue_cache_temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "jcode-desktop-issue-cache-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn issue(number: u64, title: &str, labels: &[&str], comments: usize) -> CachedGitHubIssue {
        CachedGitHubIssue {
            number,
            title: title.to_string(),
            body: Some(format!("body for {title}")),
            labels: labels
                .iter()
                .map(|name| CachedGitHubLabel {
                    name: (*name).to_string(),
                })
                .collect(),
            comment_count: Some(comments as u32),
            comments: (0..comments)
                .map(|index| CachedGitHubComment {
                    author: Some(format!("user{index}")),
                    body: format!("comment {index}"),
                    created_at: None,
                })
                .collect(),
            state: Some("open".to_string()),
            created_at: Some("2026-05-01".to_string()),
            updated_at: None,
            assignees: Vec::new(),
            milestone: None,
        }
    }

    #[test]
    fn parses_common_github_remote_urls() {
        assert_eq!(
            parse_github_repo_from_remote("git@github.com:1jehuang/jcode.git").as_deref(),
            Some("1jehuang/jcode")
        );
        assert_eq!(
            parse_github_repo_from_remote("https://github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            parse_github_repo_from_remote("ssh://git@github.com/owner/repo").as_deref(),
            Some("owner/repo")
        );
        assert!(parse_github_repo_from_remote("https://example.com/owner/repo").is_none());
    }

    #[test]
    fn ranks_explicit_priority_and_local_overrides_first() {
        let cache = GitHubIssueCache {
            schema_version: ISSUE_CACHE_SCHEMA_VERSION,
            repo: "owner/repo".to_string(),
            synced_at: Some("now".to_string()),
            issues: vec![
                issue(10, "normal cleanup", &["enhancement"], 0),
                issue(11, "crash regression", &["bug", "regression"], 1),
                issue(12, "user override", &["docs"], 0),
            ],
            local_overrides: vec![CachedGitHubIssueOverride {
                number: 12,
                priority: Some("P0".to_string()),
                pinned: true,
            }],
        };
        let browser = issue_browser_from_cache(cache);
        let numbers = browser
            .issues
            .iter()
            .map(|issue| issue.number)
            .collect::<Vec<_>>();
        assert_eq!(numbers, vec![12, 11, 10]);
        assert_eq!(browser.issues[0].priority, "P0");
        assert_eq!(browser.issues[1].priority, "P1");
        assert_eq!(browser.issues[0].state, GitHubIssueVisualState::Selected);
    }

    #[test]
    fn filters_closed_issues_and_is_deterministic() {
        let mut closed = issue(20, "closed", &["P0"], 10);
        closed.state = Some("closed".to_string());
        let cache = GitHubIssueCache {
            schema_version: ISSUE_CACHE_SCHEMA_VERSION,
            repo: "owner/repo".to_string(),
            synced_at: None,
            issues: vec![
                issue(1, "same score low number", &["bug"], 0),
                issue(2, "same score high number", &["bug"], 0),
                closed,
            ],
            local_overrides: Vec::new(),
        };
        let browser = issue_browser_from_cache(cache);
        assert_eq!(
            browser
                .issues
                .iter()
                .map(|issue| issue.number)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!(browser.filter_label.contains("cached unsynced cache"));
    }

    #[test]
    fn syncs_issue_list_comments_and_preserves_local_overrides() {
        let root = unique_issue_cache_temp_root("sync-success");
        let existing = GitHubIssueCache {
            schema_version: ISSUE_CACHE_SCHEMA_VERSION,
            repo: "owner/repo".to_string(),
            synced_at: Some("old".to_string()),
            issues: vec![issue(5, "old title", &["docs"], 0)],
            local_overrides: vec![CachedGitHubIssueOverride {
                number: 5,
                priority: Some("P0".to_string()),
                pinned: true,
            }],
        };
        write_issue_cache_to_root(&existing, &root).unwrap();

        let list_output = serde_json::json!([
            {
                "number": 5,
                "title": "synced bug",
                "body": "fresh body",
                "labels": [{"name": "bug"}],
                "comments": 1,
                "state": "OPEN",
                "createdAt": "2026-05-01T00:00:00Z",
                "updatedAt": "2026-05-02T00:00:00Z",
                "assignees": [{"login": "octo"}],
                "milestone": {"title": "desktop"}
            },
            {
                "number": 6,
                "title": "normal enhancement",
                "body": "nice to have",
                "labels": [{"name": "enhancement"}],
                "comments": 0,
                "state": "OPEN",
                "createdAt": "2026-05-03T00:00:00Z",
                "updatedAt": "2026-05-03T00:00:00Z",
                "assignees": [],
                "milestone": null
            }
        ])
        .to_string();
        let comment_output = serde_json::json!({
            "comments": [{
                "author": {"login": "maintainer"},
                "body": "synced comment",
                "createdAt": "2026-05-02T01:00:00Z"
            }]
        })
        .to_string();
        let runner = FakeGhRunner::new(list_output).with_comment_output(5, comment_output);

        let summary = sync_issue_cache_for_repo_with_runner_and_root(
            "owner/repo",
            &runner,
            GitHubIssueSyncOptions {
                issue_limit: 10,
                comment_thread_limit: 10,
            },
            &root,
        )
        .unwrap();

        assert_eq!(summary.repo, "owner/repo");
        assert_eq!(summary.issue_count, 2);
        assert_eq!(summary.fetched_comment_threads, 1);
        assert_eq!(summary.comment_fetch_errors, 0);
        assert!(summary.cache_path.is_file());
        assert_eq!(summary.browser.issues[0].number, 5);
        assert_eq!(summary.browser.issues[0].priority, "P0");
        assert_eq!(summary.browser.issues[0].comments, 1);
        assert!(summary.browser.issues[0].comment_lines[0].contains("maintainer: synced comment"));

        let saved = load_issue_cache_for_repo_raw_from_root("owner/repo", &root).unwrap();
        assert_eq!(saved.local_overrides.len(), 1);
        assert_eq!(saved.local_overrides[0].number, 5);
        assert!(
            runner
                .calls()
                .iter()
                .any(|args| args.starts_with(&["issue".to_string(), "list".to_string()]))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sync_reuses_cached_comments_when_comment_fetch_fails() {
        let root = unique_issue_cache_temp_root("sync-comment-error");
        let existing = GitHubIssueCache {
            schema_version: ISSUE_CACHE_SCHEMA_VERSION,
            repo: "owner/repo".to_string(),
            synced_at: Some("old".to_string()),
            issues: vec![issue(7, "old crash", &["bug"], 1)],
            local_overrides: Vec::new(),
        };
        write_issue_cache_to_root(&existing, &root).unwrap();

        let list_output = serde_json::json!([{
            "number": 7,
            "title": "crash regression",
            "body": "crashes on launch",
            "labels": [{"name": "bug"}, {"name": "regression"}],
            "comments": 2,
            "state": "OPEN",
            "createdAt": "2026-05-01T00:00:00Z",
            "updatedAt": "2026-05-04T00:00:00Z",
            "assignees": [],
            "milestone": null
        }])
        .to_string();
        let runner = FakeGhRunner::new(list_output).with_comment_error(7, "rate limited");

        let summary = sync_issue_cache_for_repo_with_runner_and_root(
            "owner/repo",
            &runner,
            GitHubIssueSyncOptions {
                issue_limit: 10,
                comment_thread_limit: 10,
            },
            &root,
        )
        .unwrap();

        assert_eq!(summary.fetched_comment_threads, 0);
        assert_eq!(summary.comment_fetch_errors, 1);
        assert_eq!(summary.browser.issues[0].number, 7);
        assert_eq!(summary.browser.issues[0].comments, 2);
        assert!(summary.browser.issues[0].comment_lines[0].contains("user0: comment 0"));
        let _ = std::fs::remove_dir_all(root);
    }
}
