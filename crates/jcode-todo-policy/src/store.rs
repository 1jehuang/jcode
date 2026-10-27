//! On-disk layout of a session's todo state, shared by the runtime (which
//! writes it) and the harness bridge (which serves it to SDK clients).
//!
//! Everything lives under `<jcode home>/todos/`. Reads are tolerant: a missing
//! or malformed file yields the empty value, matching the runtime's loaders.

use crate::followup::{TodoEffects, TodoSnapshot};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

/// Private policy. Never include this duration in model-facing text.
pub const LONG_SESSION_REVIEW_AFTER_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoFile {
    Todos,
    Goals,
    Plan,
    GateObservations,
    ReviewState,
    /// Swarm lane links: which coordinator lane this worker session owns, or
    /// which workers own this coordinator's lanes.
    LaneLinks,
}

impl TodoFile {
    fn suffix(self) -> &'static str {
        match self {
            Self::Todos => "",
            Self::Goals => "-goals",
            Self::Plan => "-plan",
            Self::GateObservations => "-gate-observations",
            Self::ReviewState => "-review-state",
            Self::LaneLinks => "-lane-links",
        }
    }
}

/// Whether `session_id` is safe to interpolate into a path.
pub fn is_safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub fn todo_file_path(home: &Path, session_id: &str, file: TodoFile) -> PathBuf {
    home.join("todos")
        .join(format!("{session_id}{}.json", file.suffix()))
}

/// Persisted long-session review clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoReviewState {
    pub cycle_started_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub review_delivered: bool,
}

impl TodoReviewState {
    pub fn is_due(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        !self.review_delivered
            && now - self.cycle_started_at
                >= chrono::Duration::seconds(LONG_SESSION_REVIEW_AFTER_SECS)
    }
}

fn read_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Prepare a session's todos for saving. Linked workers always retain their
/// coordinator-assigned lane identity, including in their own persisted list.
/// Coordinator-only links and unlinked sessions do not constrain authored lanes.
pub fn canonicalize_worker_todos(
    home: &Path,
    session_id: &str,
    todos: &[crate::TodoItem],
) -> Vec<crate::TodoItem> {
    let links: Vec<crate::stages::LaneLink> =
        read_or_default(&todo_file_path(home, session_id, TodoFile::LaneLinks));
    let Some(link) = links.iter().find(|link| link.worker == session_id) else {
        return todos.to_vec();
    };
    if !is_safe_session_id(&link.coordinator) {
        return todos.to_vec();
    }
    let coordinator_todos = read_or_default::<Vec<crate::TodoItem>>(&todo_file_path(
        home,
        &link.coordinator,
        TodoFile::Todos,
    ));
    crate::stages::canonicalize_worker_lane(&coordinator_todos, link, todos)
}

/// Read everything the follow-up policy needs. Never fails: absent state is
/// an empty snapshot.
pub fn load_snapshot(home: &Path, session_id: &str) -> TodoSnapshot {
    load_snapshot_with_background_dir(
        home,
        session_id,
        &std::env::temp_dir().join("jcode-bg-tasks"),
    )
}

fn load_snapshot_with_background_dir(
    home: &Path,
    session_id: &str,
    background_dir: &Path,
) -> TodoSnapshot {
    let path = |file| todo_file_path(home, session_id, file);
    let review: Option<TodoReviewState> = std::fs::read_to_string(path(TodoFile::ReviewState))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let todos: Vec<crate::TodoItem> = read_or_default(&path(TodoFile::Todos));
    let running_background_tasks = running_dependencies(&todos, session_id, background_dir);
    TodoSnapshot {
        todos,
        plan: read_or_default(&path(TodoFile::Plan)),
        goals: read_or_default(&path(TodoFile::Goals)),
        gate_observations: read_or_default(&path(TodoFile::GateObservations)),
        long_session_review_due: review.is_some_and(|state| state.is_due(chrono::Utc::now())),
        running_background_tasks,
    }
}

/// Read only referenced tasks, never unrelated sessions' background state.
/// The daemon owns status reconciliation. Missing/malformed/terminal records
/// are deliberately actionable rather than an excuse to wait forever.
fn running_dependencies(
    todos: &[crate::TodoItem],
    session_id: &str,
    directory: &Path,
) -> Vec<String> {
    let mut running = std::collections::BTreeSet::new();
    for id in todos.iter().flat_map(|todo| &todo.waiting_on) {
        if crate::await_dependency::running(&jcode_storage::runtime_dir(), id, session_id) {
            running.insert(id.clone());
            continue;
        }
        if !is_safe_session_id(id) {
            continue;
        }
        let status: serde_json::Value =
            read_or_default(&directory.join(format!("{id}.status.json")));
        if status["task_id"].as_str() == Some(id)
            && status["session_id"].as_str() == Some(session_id)
            && status["status"].as_str() == Some("running")
        {
            // Without an identifiable owner a historical running record is
            // not proof that work still exists. Fail open like the CLI.
            let owner = if status["detached"].as_bool() == Some(true) {
                status["pid"].as_u64()
            } else {
                status["owner_pid"]
                    .as_u64()
                    .or_else(|| status["pid"].as_u64())
            };
            let Some(owner) = owner.filter(|pid| *pid > 0 && *pid <= i32::MAX as u64) else {
                continue;
            };
            // Linux exposes dead owners without coupling this small policy
            // crate to the application runtime.
            #[cfg(target_os = "linux")]
            {
                if !Path::new("/proc").join(owner.to_string()).exists() {
                    continue;
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = owner;
            running.insert(id.clone());
        }
    }
    running.into_iter().collect()
}

/// Apply the persistent side effects of a follow-up decision.
pub fn apply_effects(home: &Path, session_id: &str, effects: TodoEffects) -> std::io::Result<()> {
    if effects.clear_gate_observations {
        match std::fs::remove_file(todo_file_path(home, session_id, TodoFile::GateObservations)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if effects.mark_long_session_review_delivered {
        let path = todo_file_path(home, session_id, TodoFile::ReviewState);
        if let Some(mut state) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<TodoReviewState>(&text).ok())
        {
            state.review_delivered = true;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&state)?)?;
            std::fs::rename(tmp, path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GateObservation, GateObservationKind, TodoItem};

    #[test]
    fn worker_replacement_persists_coordinator_lane_and_phase_on_both_sides() {
        let dir = std::env::temp_dir().join(format!("todo-canonical-lane-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("todos")).unwrap();
        let write = |session, file, value: serde_json::Value| {
            std::fs::write(todo_file_path(&dir, session, file), value.to_string()).unwrap();
        };
        let assigned = TodoItem {
            id: "assigned".into(),
            content: "original assignment".into(),
            status: "pending".into(),
            group: Some("automatic assignment reports".into()),
            stage: Some(2),
            ..Default::default()
        };
        let unrelated = TodoItem {
            id: "other".into(),
            group: Some("unrelated".into()),
            stage: Some(3),
            ..Default::default()
        };
        let coordinator = vec![assigned.clone(), unrelated.clone()];
        let link = crate::stages::LaneLink {
            coordinator: "root".into(),
            worker: "worker".into(),
            lane: crate::stages::lane_id(&assigned),
        };
        write(
            "root",
            TodoFile::Todos,
            serde_json::to_value(&coordinator).unwrap(),
        );
        for session in ["root", "worker"] {
            write(
                session,
                TodoFile::LaneLinks,
                serde_json::to_value([&link]).unwrap(),
            );
        }
        // A full phases replacement can change every item id, lane and phase.
        let replacement = vec![TodoItem {
            id: "replacement".into(),
            content: "updated task content".into(),
            status: "completed".into(),
            details: Some("worker-authored details".into()),
            group: Some("server output reports".into()),
            stage: Some(9),
            completely_blocked: None,
            external_blocker: None,
            waiting_on: vec!["worker-task".into()],
            ..Default::default()
        }];
        let canonical = canonicalize_worker_todos(&dir, "worker", &replacement);
        let mut expected = replacement[0].clone();
        expected.group = assigned.group.clone();
        expected.stage = assigned.stage;
        assert_eq!(canonical, [expected]);
        write(
            "worker",
            TodoFile::Todos,
            serde_json::to_value(&canonical).unwrap(),
        );
        let persisted_worker = load_snapshot(&dir, "worker").todos;
        assert_eq!(persisted_worker, canonical);
        let merged =
            crate::stages::merge_worker_lane(&coordinator, &link, &persisted_worker).unwrap();
        write(
            "root",
            TodoFile::Todos,
            serde_json::to_value(&merged).unwrap(),
        );
        let persisted_root = load_snapshot(&dir, "root").todos;
        assert_eq!(persisted_root[1], unrelated);
        assert_eq!(persisted_root[0].group, persisted_worker[0].group);
        assert_eq!(persisted_root[0].stage, Some(2));
        assert_eq!(persisted_root[0].content, replacement[0].content);
        assert_eq!(persisted_root[0].status, "completed");
        assert_eq!(
            canonicalize_worker_todos(&dir, "worker", &canonical),
            canonical
        );
        assert_eq!(
            canonicalize_worker_todos(&dir, "root", &replacement),
            replacement
        );
        assert_eq!(
            canonicalize_worker_todos(&dir, "unlinked", &replacement),
            replacement
        );
        std::fs::remove_file(todo_file_path(&dir, "root", TodoFile::Todos)).unwrap();
        assert_eq!(
            canonicalize_worker_todos(&dir, "worker", &replacement),
            replacement
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshots_include_only_referenced_running_tasks_in_the_same_session() {
        let dir =
            std::env::temp_dir().join(format!("todo-background-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("todos")).unwrap();
        let tasks = dir.join("background");
        std::fs::create_dir_all(&tasks).unwrap();
        let todo = crate::TodoItem {
            id: "validate".into(),
            status: "pending".into(),
            completely_blocked: None,
            external_blocker: None,
            waiting_on: vec![
                "running".into(),
                "finished".into(),
                "failed".into(),
                "foreign".into(),
                "missing".into(),
                "broken".into(),
                "unowned".into(),
                "../escape".into(),
                "running".into(),
            ],
            ..Default::default()
        };
        std::fs::write(
            todo_file_path(&dir, "s", TodoFile::Todos),
            serde_json::to_string(&[todo]).unwrap(),
        )
        .unwrap();
        for (id, session, status) in [
            ("running", "s", "running"),
            ("finished", "s", "completed"),
            ("failed", "s", "failed"),
            ("foreign", "other", "running"),
            ("unreferenced", "s", "running"),
        ] {
            std::fs::write(
                tasks.join(format!("{id}.status.json")),
                serde_json::json!({
                    "task_id": id, "session_id": session, "status": status,
                    "owner_pid": std::process::id()
                })
                .to_string(),
            )
            .unwrap();
        }
        std::fs::write(tasks.join("broken.status.json"), "not json").unwrap();
        std::fs::write(
            tasks.join("unowned.status.json"),
            serde_json::json!({
                "task_id": "unowned", "session_id": "s", "status": "running"
            })
            .to_string(),
        )
        .unwrap();
        let state = load_snapshot_with_background_dir(&dir, "s", &tasks);
        assert_eq!(state.running_background_tasks, ["running"]);
        std::fs::remove_file(tasks.join("running.status.json")).unwrap();
        assert!(
            load_snapshot_with_background_dir(&dir, "s", &tasks)
                .running_background_tasks
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layout_matches_runtime_file_names() {
        let home = Path::new("/h");
        assert_eq!(
            todo_file_path(home, "s", TodoFile::Todos),
            Path::new("/h/todos/s.json")
        );
        assert_eq!(
            todo_file_path(home, "s", TodoFile::GateObservations),
            Path::new("/h/todos/s-gate-observations.json")
        );
        assert!(!is_safe_session_id("../x"));
        assert!(is_safe_session_id("session_a_1_ff"));
    }

    #[test]
    fn snapshot_reads_state_and_effects_consume_it() {
        let dir = std::env::temp_dir().join(format!("todo-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("todos")).unwrap();
        let write = |file, value: serde_json::Value| {
            std::fs::write(todo_file_path(&dir, "s", file), value.to_string()).unwrap()
        };
        let todos = vec![TodoItem {
            content: "a".into(),
            status: "pending".into(),
            ..Default::default()
        }];
        write(TodoFile::Todos, serde_json::to_value(&todos).unwrap());
        let observation = GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: None,
            state: None,
        };
        write(
            TodoFile::GateObservations,
            serde_json::to_value([&observation]).unwrap(),
        );
        let old = TodoReviewState {
            cycle_started_at: chrono::Utc::now() - chrono::Duration::hours(2),
            review_delivered: false,
        };
        write(TodoFile::ReviewState, serde_json::to_value(&old).unwrap());

        let snapshot = load_snapshot(&dir, "s");
        assert_eq!(snapshot.todos, todos);
        assert_eq!(snapshot.gate_observations, vec![observation]);
        assert!(snapshot.long_session_review_due);

        apply_effects(
            &dir,
            "s",
            TodoEffects {
                clear_gate_observations: true,
                mark_long_session_review_delivered: true,
            },
        )
        .unwrap();
        let after = load_snapshot(&dir, "s");
        assert!(after.gate_observations.is_empty());
        assert!(!after.long_session_review_due);
        // Idempotent.
        apply_effects(
            &dir,
            "s",
            TodoEffects {
                clear_gate_observations: true,
                mark_long_session_review_delivered: true,
            },
        )
        .unwrap();
        assert_eq!(load_snapshot(&dir, "missing"), TodoSnapshot::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
