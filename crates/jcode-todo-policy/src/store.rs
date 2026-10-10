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
}

impl TodoFile {
    fn suffix(self) -> &'static str {
        match self {
            Self::Todos => "",
            Self::Goals => "-goals",
            Self::Plan => "-plan",
            Self::GateObservations => "-gate-observations",
            Self::ReviewState => "-review-state",
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

/// Read everything the follow-up policy needs. Never fails: absent state is
/// an empty snapshot.
pub fn load_snapshot(home: &Path, session_id: &str) -> TodoSnapshot {
    let path = |file| todo_file_path(home, session_id, file);
    let review: Option<TodoReviewState> = std::fs::read_to_string(path(TodoFile::ReviewState))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    TodoSnapshot {
        todos: read_or_default(&path(TodoFile::Todos)),
        plan: read_or_default(&path(TodoFile::Plan)),
        goals: read_or_default(&path(TodoFile::Goals)),
        gate_observations: read_or_default(&path(TodoFile::GateObservations)),
        long_session_review_due: review.is_some_and(|state| state.is_due(chrono::Utc::now())),
    }
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
