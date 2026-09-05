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
    #[serde(rename = "gitStatus")]
    pub git_status: Option<String>,
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
    let git_status = Command::new("git")
        .args(["status", "--short"])
        .current_dir(&cwd)
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .chars()
                .take(2048)
                .collect()
        });
    let commits = Command::new("git")
        .args(["log", "-8", "--pretty=format:%s"])
        .current_dir(&cwd)
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_owned)
                .take(8)
                .collect()
        })
        .unwrap_or_default();
    let value = ProjectContext {
        cwd: cwd.display().to_string(),
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        environment: std::env::consts::OS.to_string(),
        structure: std::fs::read_dir(&cwd)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .take(64)
            .collect(),
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
