//! `jcode recall` — reconstruct recent working context from sessions, todos, git, and memories.
//!
//! Emits a compact capsule of where work stands: recent sessions for this project,
//! open todos from the latest session, git branch state, and relevant memories.
//! No edits, just a clean summary for starting fresh or catching up.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command as StdCommand;

#[allow(dead_code)]
const DEFAULT_SINCE_DAYS: i64 = 3;

/// Recall report for a working directory.
#[derive(Debug, Serialize)]
pub struct RecallReport {
    pub working_dir: String,
    pub since_days: i64,
    pub generated_at: String,
    pub git: GitState,
    pub sessions: Vec<SessionSummary>,
    pub memories: Vec<MemorySummary>,
    pub todos: Vec<TodoSummary>,
    pub prs_open: usize,
}

/// Recent git state.
#[derive(Debug, Serialize)]
pub struct GitState {
    pub branch: String,
    pub recent_commits: Vec<GitCommit>,
    pub has_uncommitted: bool,
}

/// One recent commit.
#[derive(Debug, Serialize)]
pub struct GitCommit {
    pub hash: String,
    pub date: String,
    pub message: String,
}

/// One recent session stub.
#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub message_count: usize,
}

/// One project memory entry.
#[derive(Debug, Serialize)]
pub struct MemorySummary {
    pub id: String,
    pub category: String,
    pub content: String,
    pub created_at: String,
    pub tags: Vec<String>,
}

/// One open todo item.
#[derive(Debug, Serialize)]
pub struct TodoSummary {
    pub id: String,
    pub content: String,
    pub priority: String,
    pub status: String,
}

/// Run the recall command.
pub fn run_recall_command(working_dir: Option<&str>, since_days: i64, format: &str) -> Result<()> {
    let working_dir = match working_dir {
        Some(wd) => wd.to_string(),
        None => std::env::current_dir()
            .context("no --working-dir given and cwd unavailable")?
            .to_string_lossy()
            .to_string(),
    };

    let since = Utc::now() - chrono::Duration::days(since_days);

    // Git state.
    let git = git_state(&working_dir);

    // Recent sessions for this working dir.
    let sessions = recent_sessions_for_dir(&working_dir, since)?;

    // Todos from the most recent session.
    let todos = if let Some(sess) = sessions.first() {
        load_todos_for_session(&sess.id)?
    } else {
        Vec::new()
    };

    // Project memories.
    let memories = load_project_memories(&working_dir)?;

    // Open PR count.
    let prs_open = count_open_prs()?;

    let report = RecallReport {
        working_dir: working_dir.clone(),
        since_days,
        generated_at: Utc::now().to_rfc3339(),
        git,
        sessions,
        memories,
        todos,
        prs_open,
    };

    match format {
        "json" => {
            serde_json::to_writer_pretty(std::io::stdout(), &report)?;
            println!();
        }
        _ => print_text(&report),
    }

    Ok(())
}

// ─── Git ─────────────────────────────────────────────────────────────────────

pub(crate) fn git_state(working_dir: &str) -> GitState {
    let repo = Path::new(working_dir);
    if !repo.join(".git").exists() {
        return GitState {
            branch: String::new(),
            recent_commits: Vec::new(),
            has_uncommitted: false,
        };
    }

    let branch = StdCommand::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let has_uncommitted = StdCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo)
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let recent_commits: Vec<GitCommit> = StdCommand::new("git")
        .args([
            "log",
            "--since=30 days ago",
            "--oneline",
            "-20",
            "--format=%H %cd %s",
            "--date=iso",
        ])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, ' ');
                    let hash = parts.next()?.trim().to_string();
                    let date = parts.next()?.trim().to_string();
                    let message = parts.next().unwrap_or("").trim().to_string();
                    Some(GitCommit {
                        hash: hash[..7.min(hash.len())].to_string(),
                        date,
                        message,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    GitState {
        branch,
        recent_commits,
        has_uncommitted,
    }
}

// ─── Sessions ─────────────────────────────────────────────────────────────────

fn recent_sessions_for_dir(working_dir: &str, since: DateTime<Utc>) -> Result<Vec<SessionSummary>> {
    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let root = Path::new(working_dir);
    let mut found = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

        // Parse sessions.  Small files: load_startup_stub (fast, no transcript).
        // Large files: load() (reads stub, skips transcript, safe).
        // Both carry working_dir.  Errors are swallowed since many session files are
        // corrupt or in-flight; we only need the ones that parse cleanly.
        let stub = if meta.len() < 50_000 {
            crate::session::Session::load_startup_stub(stem)
                .with_context(|| format!("loading session stub: {}", path.display()))
                .ok()
        } else {
            crate::session::Session::load(stem)
                .with_context(|| format!("loading session: {}", path.display()))
                .ok()
        };
        let Some(stub) = stub else {
            continue;
        };

        if stub.updated_at < since {
            continue;
        }

        // Match working_dir.
        let matches = stub.working_dir.as_ref().is_some_and(|wd| {
            Path::new(wd.as_str()).starts_with(root)
        });

        if !matches {
            continue;
        }

        let title = stub
            .custom_title
            .clone()
            .or(stub.title.clone())
            .unwrap_or_else(|| stub.id.clone());

        found.push(SessionSummary {
            id: stub.id.clone(),
            title,
            updated_at: stub.updated_at.to_rfc3339(),
            message_count: 0, // Stub doesn't carry count; omit for perf.
        });
    }

    // Sort newest first.
    found.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    // Deduplicate by session id (multiple .json variants exist for the same session).
    let mut seen = std::collections::HashSet::new();
    found.retain(|s| seen.insert(s.id.clone()));

    Ok(found)
}

// ─── Todos ───────────────────────────────────────────────────────────────────

pub(crate) fn load_todos_for_session(session_id: &str) -> Result<Vec<TodoSummary>> {
    let base = crate::storage::jcode_dir()?;
    let path = base.join("todos").join(format!("{}.json", session_id));

    if !path.exists() {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    struct TodoEntry {
        id: String,
        content: String,
        #[serde(default)]
        priority: Option<serde_json::Value>,
        status: String,
    }

    // Todo files are a top-level JSON array (crate::todo::TodoItem list).
    let todos: Vec<TodoEntry> = serde_json::from_reader(std::fs::File::open(&path)?)
        .with_context(|| format!("parsing todos: {}", path.display()))?;

    let open: Vec<TodoSummary> = todos
        .into_iter()
        .filter(|t| t.status != "completed")
        .map(|t| TodoSummary {
            id: t.id,
            content: t.content,
            priority: match t.priority {
                Some(serde_json::Value::String(s)) => s,
                Some(other) => other.to_string(),
                None => "normal".to_string(),
            },
            status: t.status,
        })
        .collect();

    Ok(open)
}

// ─── Memories ─────────────────────────────────────────────────────────────────

pub(crate) fn load_project_memories(working_dir: &str) -> Result<Vec<MemorySummary>> {
    let path = crate::memory::project_memory_file(Path::new(working_dir))?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    struct MemStore {
        memories: BTreeMap<String, MemEntry>,
    }
    #[derive(serde::Deserialize)]
    struct MemEntry {
        id: String,
        category: jcode_memory_types::MemoryCategory,
        content: String,
        created_at: String,
        #[serde(default)]
        tags: Vec<String>,
    }

    let store: MemStore = serde_json::from_reader(std::fs::File::open(&path)?)
        .with_context(|| format!("parsing memories: {}", path.display()))?;

    let summaries: Vec<MemorySummary> = store
        .memories
        .into_iter()
        .map(|(_, e)| MemorySummary {
            id: e.id,
            category: e.category.to_string(),
            content: e.content,
            created_at: e.created_at,
            tags: e.tags,
        })
        .collect();

    // Sort newest first by id (id encodes timestamp).
    let mut summaries = summaries;
    summaries.sort_by(|a, b| b.id.cmp(&a.id));
    summaries.truncate(20); // Cap at 20 most recent memories.

    Ok(summaries)
}

// ─── PRs ─────────────────────────────────────────────────────────────────────

fn count_open_prs() -> Result<usize> {
    let output = StdCommand::new("gh")
        .args(["pr", "list", "--state", "open", "--json", "number"])
        .output()
        .context("failed to run `gh pr list`")?;

    if !output.status.success() {
        return Ok(0); // Not authenticated or not a git repo.
    }

    let arr: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).unwrap_or_default();

    Ok(arr.len())
}

// ─── Output ───────────────────────────────────────────────────────────────────

fn print_text(report: &RecallReport) {
    println!("=== Recall: {} ({}) ===", report.working_dir, report.generated_at);

    // Git.
    if !report.git.branch.is_empty() {
        let dirty = if report.git.has_uncommitted { " (uncommitted changes)" } else { "" };
        println!("Git: {} on branch '{}{}'", report.git.recent_commits.len(), report.git.branch, dirty);
        for c in report.git.recent_commits.iter().take(5) {
            println!("  {} {}  {}", c.hash, c.date.split_whitespace().next().unwrap_or(""), c.message);
        }
    } else {
        println!("Git: not a git repo");
    }

    // Sessions.
    println!("\nRecent sessions ({} days):", report.since_days);
    if report.sessions.is_empty() {
        println!("  none");
    } else {
        for s in report.sessions.iter().take(5) {
            let date = s.updated_at.split('T').next().unwrap_or(&s.updated_at);
            println!("  {}  {}", date, s.title);
        }
    }

    // Todos.
    println!("\nOpen todos ({}) :", report.todos.len());
    if report.todos.is_empty() {
        println!("  none");
    } else {
        for t in &report.todos {
            println!("  [{}] {}", t.priority, t.content);
        }
    }

    // Memories.
    println!("\nProject memories ({} recent):", report.memories.len());
    if report.memories.is_empty() {
        println!("  none");
    } else {
        for m in &report.memories {
            let preview = m.content.chars().take(80).collect::<String>();
            let ellipsis = if m.content.len() > 80 { "..." } else { "" };
            println!("  [{}] {}{}", m.category, preview, ellipsis);
        }
    }

    // PRs.
    if report.prs_open > 0 {
        println!("\nOpen PRs: {} (use `jcode pr` for detail)", report.prs_open);
    }
}

#[cfg(test)]
#[allow(unused_fields)]
mod tests {
    use super::*;

    #[test]
    fn git_state_nonexistent_dir() {
        let state = git_state("/nonexistent/path/that/does/not/exist");
        assert!(state.branch.is_empty());
        assert!(state.recent_commits.is_empty());
        assert!(!state.has_uncommitted);
    }

    // ─── Todos ────────────────────────────────────────────────────────────────

    #[test]
    fn load_todos_empty_array() {
        let json = "[]";
        #[derive(serde::Deserialize)]
        struct TodoEntry {
            id: String,
            content: String,
            #[serde(default)]
            priority: Option<serde_json::Value>,
            status: String,
        }
        let todos: Vec<TodoEntry> = serde_json::from_str(json).unwrap();
        assert!(todos.is_empty());
    }

    #[test]
    fn load_todos_string_priority() {
        // Priority can be a string.
        let json = r#"[{"id":"a","content":"do thing","priority":"high","status":"in_progress"}]"#;
        #[derive(serde::Deserialize)]
        struct TodoEntry {
            id: String,
            content: String,
            #[serde(default)]
            priority: Option<serde_json::Value>,
            status: String,
        }
        let todos: Vec<TodoEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].priority.as_ref().unwrap().as_str().unwrap(), "high");
    }

    #[test]
    fn load_todos_value_priority() {
        // Priority can be a non-string Value (e.g. null or number).
        let json = r#"[{"id":"b","content":"other thing","priority":null,"status":"pending"}]"#;
        #[derive(serde::Deserialize)]
        struct TodoEntry {
            id: String,
            content: String,
            #[serde(default)]
            priority: Option<serde_json::Value>,
            status: String,
        }
        let todos: Vec<TodoEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(todos.len(), 1);
        assert!(todos[0].priority.is_none());
    }

    #[test]
    fn load_todos_missing_priority_field() {
        // Some todo files omit priority entirely.
        let json = r#"[{"id":"c","content":"no priority","status":"in_progress"}]"#;
        #[derive(serde::Deserialize)]
        struct TodoEntry {
            id: String,
            content: String,
            #[serde(default)]
            priority: Option<serde_json::Value>,
            status: String,
        }
        let todos: Vec<TodoEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(todos.len(), 1);
        assert!(todos[0].priority.is_none());
    }

    // ─── Memories ────────────────────────────────────────────────────────────

    #[test]
    fn load_project_memories_string_category() {
        // Standard memory with string category.
        let json = r#"{
            "memories": {
                "mem_test1": {
                    "id": "mem_test1",
                    "category": "fact",
                    "content": "hello world",
                    "created_at": "2026-01-01T00:00:00Z",
                    "tags": ["tag1"]
                }
            }
        }"#;
        #[derive(serde::Deserialize)]
        struct MemStore {
            memories: std::collections::BTreeMap<String, MemEntry>,
        }
        #[allow(unused_fields)]
        #[derive(serde::Deserialize)]
        struct MemEntry {
            id: String,
            category: jcode_memory_types::MemoryCategory,
            content: String,
            created_at: String,
            #[serde(default)]
            tags: Vec<String>,
        }
        let store: MemStore = serde_json::from_str(json).unwrap();
        let entry = store.memories.get("mem_test1").unwrap();
        assert_eq!(entry.category.to_string(), "fact");
    }

    #[test]
    fn load_project_memories_custom_category() {
        // Custom category serializes as {"custom": "..."}.
        let json = r#"{
            "memories": {
                "mem_test2": {
                    "id": "mem_test2",
                    "category": {"custom":"**correction**"},
                    "content": "correction content",
                    "created_at": "2026-01-01T00:00:00Z",
                    "tags": []
                }
            }
        }"#;
        #[derive(serde::Deserialize)]
        struct MemStore {
            memories: std::collections::BTreeMap<String, MemEntry>,
        }
        #[allow(unused_fields)]
        #[derive(serde::Deserialize)]
        struct MemEntry {
            id: String,
            category: jcode_memory_types::MemoryCategory,
            content: String,
            created_at: String,
            #[serde(default)]
            tags: Vec<String>,
        }
        let store: MemStore = serde_json::from_str(json).unwrap();
        let entry = store.memories.get("mem_test2").unwrap();
        assert_eq!(entry.category.to_string(), "**correction**");
    }

    #[test]
    fn load_project_memories_missing_tags() {
        // Old memory files may omit tags entirely.
        let json = r#"{
            "memories": {
                "mem_test3": {
                    "id": "mem_test3",
                    "category": "preference",
                    "content": "pref content",
                    "created_at": "2026-01-01T00:00:00Z"
                }
            }
        }"#;
        #[derive(serde::Deserialize)]
        struct MemStore {
            memories: std::collections::BTreeMap<String, MemEntry>,
        }
        #[allow(unused_fields)]
        #[derive(serde::Deserialize)]
        struct MemEntry {
            id: String,
            category: jcode_memory_types::MemoryCategory,
            content: String,
            created_at: String,
            #[serde(default)]
            tags: Vec<String>,
        }
        let store: MemStore = serde_json::from_str(json).unwrap();
        let entry = store.memories.get("mem_test3").unwrap();
        assert!(entry.tags.is_empty());
    }
}
