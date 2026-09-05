use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProjectContext {
    #[serde(rename = "workingDir")]
    pub cwd: String,
    pub date: String,
    pub environment: String,
    pub structure: Vec<String>,
    #[serde(rename = "isGitRepo")]
    pub is_git_repo: bool,
    #[serde(rename = "currentBranch")]
    pub current_branch: String,
    #[serde(rename = "mainBranch")]
    pub main_branch: String,
    #[serde(rename = "gitStatus")]
    pub git_status: String,
    #[serde(rename = "recentCommits")]
    pub commits: Vec<String>,
    #[serde(skip)]
    pub entries: Vec<String>,
    pub agents: Option<String>,
}
static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, ProjectContext)>>> = OnceLock::new();
pub fn project_context_cache(cwd: impl AsRef<Path>) -> ProjectContext {
    let cwd = cwd.as_ref().to_path_buf();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(lock) = cache.lock() {
        if let Some((at, value)) = lock.get(&cwd) {
            if at.elapsed() < Duration::from_secs(30) {
                return value.clone();
            }
        }
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&cwd)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let is_git_repo = git(&["rev-parse", "--show-toplevel"]).is_some();
    let current_branch = is_git_repo
        .then(|| git(&["rev-parse", "--abbrev-ref", "HEAD"]))
        .flatten()
        .unwrap_or_default();
    let main_branch = is_git_repo
        .then(|| git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]))
        .flatten()
        .map(|branch| {
            branch
                .strip_prefix("origin/")
                .unwrap_or(&branch)
                .to_string()
        })
        .unwrap_or_else(|| current_branch.clone());
    let git_status = is_git_repo
        .then(|| git(&["status", "--porcelain"]))
        .flatten()
        .unwrap_or_default()
        .chars()
        .take(2048)
        .collect();
    let commits = is_git_repo
        .then(|| git(&["log", "--oneline", "-8"]))
        .flatten()
        .unwrap_or_default()
        .lines()
        .take(8)
        .map(|line| line.chars().take(512).collect())
        .collect();
    let mut structure = std::fs::read_dir(&cwd)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect::<Vec<_>>();
    structure.sort();
    structure.truncate(64);
    let value = ProjectContext {
        cwd: cwd.display().to_string(),
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        environment: std::env::consts::OS.to_string(),
        structure,
        is_git_repo,
        current_branch,
        main_branch,
        git_status,
        commits,
        entries: Vec::new(),
        agents: read_agents(&cwd),
    };
    if let Ok(mut lock) = cache.lock() {
        lock.insert(cwd, (Instant::now(), value.clone()));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_git_context_serializes_required_command_code_config_fields() {
        let dir = tempfile::tempdir().unwrap();
        let value = serde_json::to_value(project_context_cache(dir.path())).unwrap();
        assert_eq!(value["isGitRepo"], false);
        assert_eq!(value["currentBranch"], "");
        assert_eq!(value["mainBranch"], "");
        assert_eq!(value["gitStatus"], "");
        assert!(value["recentCommits"].as_array().unwrap().is_empty());
    }
}
fn read_agents(cwd: &Path) -> Option<String> {
    let mut path = Some(cwd);
    let mut out = String::new();
    while let Some(dir) = path {
        let file = dir.join("AGENTS.md");
        if let Ok(text) = std::fs::read_to_string(file) {
            out.push_str(&text);
            if out.len() >= 32768 {
                out.truncate(out.floor_char_boundary(32768));
                break;
            }
        }
        path = dir.parent();
    }
    (!out.is_empty()).then_some(out)
}
