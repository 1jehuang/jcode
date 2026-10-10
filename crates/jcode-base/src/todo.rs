//! Todo persistence plus a re-export of the shared pure policy in
//! `jcode-todo-policy`, so existing `crate::todo::*` callers are unchanged.

use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use jcode_todo_policy::*;

use jcode_todo_policy::store::{TodoFile, TodoReviewState, todo_file_path};

#[cfg(test)]
const TODO_LONG_SESSION_REVIEW_AFTER: chrono::Duration =
    chrono::Duration::seconds(jcode_todo_policy::store::LONG_SESSION_REVIEW_AFTER_SECS);

fn todo_state_path(session_id: &str, file: TodoFile) -> Result<PathBuf> {
    Ok(todo_file_path(&storage::jcode_dir()?, session_id, file))
}

pub fn load_todos(session_id: &str) -> Result<Vec<TodoItem>> {
    let path = todo_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    storage::read_json(&path).or_else(|_| Ok(Vec::new()))
}

pub fn todos_exist(session_id: &str) -> Result<bool> {
    Ok(todo_path(session_id)?.exists())
}

pub fn save_todos(session_id: &str, todos: &[TodoItem]) -> Result<()> {
    let path = todo_path(session_id)?;
    storage::write_json_fast(&path, todos)?;
    if let Err(error) = crate::recent_session_index::refresh_todo_title(session_id) {
        crate::logging::warn(&format!(
            "Failed to refresh indexed todo title for {session_id}: {error}"
        ));
    }
    Ok(())
}

fn todo_path(session_id: &str) -> Result<PathBuf> {
    todo_state_path(session_id, TodoFile::Todos)
}

fn todo_review_path(session_id: &str) -> Result<PathBuf> {
    todo_state_path(session_id, TodoFile::ReviewState)
}

/// Record the beginning of a fresh todo cycle without exposing timing metadata
/// through the model-facing todo payload. Replacing a fully completed list with
/// new open work starts a new cycle; ordinary edits retain the original clock.
pub fn update_todo_review_cycle(
    session_id: &str,
    previous: &[TodoItem],
    incoming: &[TodoItem],
) -> Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    let path = todo_review_path(session_id)?;
    let previous_complete = !previous.is_empty()
        && previous
            .iter()
            .all(|todo| todo.status.eq_ignore_ascii_case("completed"));
    let incoming_has_open = incoming
        .iter()
        .any(|todo| !todo.status.eq_ignore_ascii_case("completed"));
    if !path.exists() || (previous_complete && incoming_has_open) {
        storage::write_json_fast(
            &path,
            &TodoReviewState {
                cycle_started_at: chrono::Utc::now(),
                review_delivered: false,
            },
        )?;
    }
    Ok(())
}

/// Atomically decide and mark whether the one-shot long-session assessment
/// review is due. Marking before queueing prevents reloads from duplicating it.
pub fn take_long_session_review_if_due(session_id: &str) -> Result<bool> {
    let path = todo_review_path(session_id)?;
    if !path.exists() {
        return Ok(false);
    }
    let mut state: TodoReviewState = storage::read_json(&path)?;
    if !state.is_due(chrono::Utc::now()) {
        return Ok(false);
    }
    state.review_delivered = true;
    storage::write_json_fast(&path, &state)?;
    Ok(true)
}

/// Goal-level assessments live beside the todo list in a separate file so the
/// todo list format (a bare `Vec<TodoItem>` array) stays readable by every
/// existing consumer.
pub fn load_goals(session_id: &str) -> Result<Vec<TodoGoal>> {
    let path = goals_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    storage::read_json(&path).or_else(|_| Ok(Vec::new()))
}

/// Load todo state for a session and derive its best title hint.
pub fn load_session_title(session_id: &str) -> Option<String> {
    let todos = load_todos(session_id).ok()?;
    let plan = load_plan(session_id).unwrap_or_default();
    derive_session_title(&todos, &plan)
}

pub fn save_goals(session_id: &str, goals: &[TodoGoal]) -> Result<()> {
    let path = goals_path(session_id)?;
    storage::write_json_fast(&path, goals)
}

fn goals_path(session_id: &str) -> Result<PathBuf> {
    todo_state_path(session_id, TodoFile::Goals)
}

/// The plan-level intent assessment lives in its own file beside the todo list
/// and per-group goals, so each format stays independently readable.
pub fn load_plan(session_id: &str) -> Result<TodoPlan> {
    let path = plan_path(session_id)?;
    if !path.exists() {
        return Ok(TodoPlan::default());
    }
    storage::read_json(&path).or_else(|_| Ok(TodoPlan::default()))
}

pub fn save_plan(session_id: &str, plan: &TodoPlan) -> Result<()> {
    let path = plan_path(session_id)?;
    storage::write_json_fast(&path, plan)?;
    if let Err(error) = crate::recent_session_index::refresh_todo_title(session_id) {
        crate::logging::warn(&format!(
            "Failed to refresh indexed todo title for {session_id}: {error}"
        ));
    }
    Ok(())
}

fn plan_path(session_id: &str) -> Result<PathBuf> {
    todo_state_path(session_id, TodoFile::Plan)
}

/// Deferred quality-check observations for the current turn.
///
/// Kept in its own file for the same reason goals and plan are: each format
/// stays independently readable. This one is turn-scoped rather than durable,
/// cleared once the digest has been delivered.
pub fn load_gate_observations(session_id: &str) -> Result<Vec<GateObservation>> {
    let path = gate_observations_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    storage::read_json(&path).or_else(|_| Ok(Vec::new()))
}

pub fn save_gate_observations(session_id: &str, observations: &[GateObservation]) -> Result<()> {
    let path = gate_observations_path(session_id)?;
    storage::write_json_fast(&path, observations)
}

/// Append this write's observations, capped so a very long iterative turn
/// cannot grow the file without bound. The digest collapses repeats anyway, so
/// dropping the oldest entries past the cap costs no information the reminder
/// would have used.
pub fn append_gate_observations(session_id: &str, new: &[GateObservation]) -> Result<()> {
    if new.is_empty() {
        return Ok(());
    }
    let mut observations = load_gate_observations(session_id).unwrap_or_default();
    observations.extend(new.iter().cloned());
    if observations.len() > MAX_GATE_OBSERVATIONS {
        let excess = observations.len() - MAX_GATE_OBSERVATIONS;
        observations.drain(0..excess);
    }
    save_gate_observations(session_id, &observations)
}

pub fn clear_gate_observations(session_id: &str) -> Result<()> {
    let path = gate_observations_path(session_id)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Upper bound on retained observations per turn.
const MAX_GATE_OBSERVATIONS: usize = 256;

fn gate_observations_path(session_id: &str) -> Result<PathBuf> {
    todo_state_path(session_id, TodoFile::GateObservations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent_observation(state: Option<IntentUnderstanding>) -> GateObservation {
        GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            state: state.map(|state| state.as_str().to_string()),
        }
    }

    fn loop_observation(group: Option<&str>, state: Option<FeedbackLoopState>) -> GateObservation {
        GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: group.map(str::to_string),
            state: state.map(|state| state.as_str().to_string()),
        }
    }

    #[test]
    fn gate_observations_round_trip_and_clear() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());

        let session = "gate-observation-round-trip";
        assert!(
            load_gate_observations(session)
                .expect("load empty")
                .is_empty()
        );
        append_gate_observations(
            session,
            &[intent_observation(Some(IntentUnderstanding::Partial))],
        )
        .expect("append");
        append_gate_observations(
            session,
            &[loop_observation(
                Some("perf"),
                Some(FeedbackLoopState::Strong),
            )],
        )
        .expect("append");
        let stored = load_gate_observations(session).expect("load");
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].kind, GateObservationKind::IntentUnderstanding);
        assert_eq!(stored[1].group.as_deref(), Some("perf"));

        clear_gate_observations(session).expect("clear");
        assert!(load_gate_observations(session).expect("reload").is_empty());
        // Clearing an absent log is not an error, since the digest path clears
        // unconditionally.
        clear_gate_observations(session).expect("clear again");

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    /// A very long turn must not grow the log without bound. Repeats collapse in
    /// the digest anyway, so dropping the oldest costs nothing it would report.
    #[test]
    fn gate_observation_log_is_capped() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());

        let session = "gate-observation-cap";
        let batch: Vec<GateObservation> = (0..MAX_GATE_OBSERVATIONS + 50)
            .map(|_| intent_observation(Some(IntentUnderstanding::Partial)))
            .collect();
        append_gate_observations(session, &batch).expect("append");
        assert_eq!(
            load_gate_observations(session).expect("load").len(),
            MAX_GATE_OBSERVATIONS
        );

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    fn todo(content: &str, status: &str, group: Option<&str>) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            id: content.to_ascii_lowercase().replace(' ', "-"),
            group: group.map(str::to_string),
            confidence: None,
            completion_confidence: None,
            confidence_history: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    #[test]
    fn long_session_review_is_private_durable_and_one_shot() {
        let _guard = storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());
        let session = "long-review-one-shot";
        let todos = vec![todo("work", "in_progress", Some("ship"))];

        update_todo_review_cycle(session, &[], &todos).expect("start cycle");
        assert!(!take_long_session_review_if_due(session).expect("fresh cycle"));

        let path = todo_review_path(session).expect("review path");
        storage::write_json_fast(
            &path,
            &TodoReviewState {
                cycle_started_at: chrono::Utc::now()
                    - TODO_LONG_SESSION_REVIEW_AFTER
                    - chrono::Duration::seconds(1),
                review_delivered: false,
            },
        )
        .expect("age cycle");
        assert!(take_long_session_review_if_due(session).expect("due review"));
        assert!(!take_long_session_review_if_due(session).expect("one shot"));
        assert!(!TODO_LONG_SESSION_REVIEW_MESSAGE.contains("30"));
        assert!(
            !TODO_LONG_SESSION_REVIEW_MESSAGE
                .to_ascii_lowercase()
                .contains("threshold")
        );
        assert!(is_auto_poke_message(TODO_LONG_SESSION_REVIEW_MESSAGE));

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn plan_intent_fields_round_trip_through_storage() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());

        let plan = TodoPlan {
            user_intention: Some("Preserve why the user requested the work".to_string()),
            understands_user_intent: Some(IntentUnderstanding::Clear),
            ..Default::default()
        };
        save_plan("user-intention-round-trip", &plan).expect("save plan");
        let stored =
            std::fs::read_to_string(plan_path("user-intention-round-trip").expect("plan path"))
                .expect("read stored plan");
        assert!(stored.contains("\"understands_user_intent\""));
        assert!(!stored.contains("\"alignment_score\""));
        assert!(!stored.contains("\"user_intention_alignment\""));

        let loaded = load_plan("user-intention-round-trip").expect("load plan");
        assert_eq!(loaded, plan);

        match previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}
