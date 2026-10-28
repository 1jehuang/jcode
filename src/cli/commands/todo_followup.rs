//! Headless continuation, dependency waiting, and completion-review policy.
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug)]
pub(super) enum RunAutoPokeFollowUp {
    Incomplete {
        count: usize,
        message: String,
    },
    ConfidenceSummary {
        total_todos: usize,
        message: String,
        confidence_spike_challenge: bool,
    },
    /// Deferred quality-check reminder for the points this turn flagged and
    /// never resolved. Delivered once, ahead of the confidence summary.
    GateDigest {
        message: String,
    },
}

pub(super) fn run_todos(session_id: &str) -> Vec<crate::todo::TodoItem> {
    match crate::todo::load_todos(session_id) {
        Ok(todos) => todos,
        Err(error) => {
            crate::logging::warn(format!(
                "Cannot load headless todos for {session_id}: {error}"
            ));
            Vec::new()
        }
    }
}

pub(super) fn run_task_is_waitable(
    task: &crate::background::TaskStatusFile,
    session_id: &str,
) -> bool {
    task.session_id == session_id
        && task.status == crate::bus::BackgroundTaskStatus::Running
        // Old status files without an identifiable owner cannot be reconciled
        // by the manager. Do not let a stale file hold a headless run forever.
        && if task.detached {
            task.pid.is_some()
        } else {
            task.owner_pid.is_some()
        }
}

pub(super) fn run_actionable_todos(
    todos: &[crate::todo::TodoItem],
    tasks: &[crate::background::TaskStatusFile],
    session_id: &str,
) -> Vec<crate::todo::TodoItem> {
    todos
        .iter()
        .filter(|todo| {
            !crate::todo::is_incomplete_todo(todo)
                || (!crate::todo::todo_completely_blocked(todo)
                    && !crate::todo::todo_has_external_blocker(todo)
                    && !crate::todo::todo_waits_on_running_task(todo, |id| {
                        crate::todo::await_dependency::running(
                            &crate::storage::runtime_dir(),
                            id,
                            session_id,
                        ) || tasks.iter().any(|task| {
                            task.task_id == id && run_task_is_waitable(task, session_id)
                        })
                    }))
        })
        .cloned()
        .collect()
}

pub(super) fn run_actionable_poke_message(
    message: String,
    todos: &[crate::todo::TodoItem],
    actionable: &[crate::todo::TodoItem],
) -> String {
    if todos.iter().any(|todo| {
        !todo.waiting_on.is_empty()
            || todo.external_blocker.is_some()
            || crate::todo::todo_completely_blocked(todo)
    }) {
        let incomplete: Vec<_> = actionable
            .iter()
            .filter(|todo| crate::todo::is_incomplete_todo(todo))
            .cloned()
            .collect();
        return crate::todo::build_actionable_poke_message(&incomplete);
    }
    message
}

pub(super) fn run_completely_blocked_notice(todos: &[crate::todo::TodoItem]) -> Option<String> {
    let blockers: Vec<_> = todos
        .iter()
        .filter(|todo| crate::todo::todo_completely_blocked(todo))
        .filter_map(|todo| {
            todo.completely_blocked
                .as_ref()
                .map(|blocker| format!("{}: {}", todo.id, blocker.reason))
        })
        .collect();
    (!blockers.is_empty()).then(|| {
        format!(
            "✋ Completely blocked. No way forward.\n{}",
            blockers.join("\n")
        )
    })
}

pub(super) fn run_blocked_notice(todos: &[crate::todo::TodoItem]) -> Option<String> {
    let notices: Vec<_> = [
        run_completely_blocked_notice(todos),
        run_external_blocker_notice(todos),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!notices.is_empty()).then(|| notices.join("\n"))
}

pub(super) fn run_external_blocker_notice(todos: &[crate::todo::TodoItem]) -> Option<String> {
    let blocked: Vec<_> = todos
        .iter()
        .filter(|todo| crate::todo::todo_has_external_blocker(todo))
        .filter_map(|todo| {
            let blocker = todo.external_blocker.as_ref()?;
            Some(format!(
                "{}: {} (review {})",
                todo.id,
                blocker.reason,
                blocker.review_at.to_rfc3339()
            ))
        })
        .collect();
    (!blocked.is_empty()).then(|| format!("Unfinished todos are externally blocked. The durable scheduler will request reassessment at their review deadlines, not mark them complete.\n{}", blocked.join("\n")))
}

/// CLI owns continuation while waiting. Unlike a daemon wake, this resumes the
/// same in-memory agent and preserves capture/NDJSON output routing. Waits live
/// outside run_once's runtime activity guard and never spend the todo budget.
pub(super) async fn prepare_run_todo_follow_up(
    manager: &crate::background::BackgroundTaskManager,
    session_id: &str,
    todos: &[crate::todo::TodoItem],
    may_resume: bool,
) -> (Vec<crate::todo::TodoItem>, Option<String>) {
    use futures::{FutureExt, StreamExt};

    let mut completion_messages = Vec::new();
    let dependencies: BTreeSet<_> = todos
        .iter()
        .filter(|todo| {
            crate::todo::is_incomplete_todo(todo) && !crate::todo::todo_completely_blocked(todo)
        })
        .flat_map(|todo| todo.waiting_on.iter().cloned())
        .filter(|id| {
            // Persisted old/hand-edited todos are not necessarily tool-validated.
            // A dependency is an ID, never a path outside the task directory.
            let mut components = Path::new(id).components();
            matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
        })
        .collect();
    loop {
        let mut tasks = Vec::new();
        let mut awaits = Vec::new();
        for id in &dependencies {
            if crate::todo::await_dependency::running(
                &crate::storage::runtime_dir(),
                id,
                session_id,
            ) {
                // This CLI owns continuation, just as for process-backed jobs.
                if let Err(error) =
                    crate::server::set_await_dependency_delivery(id, session_id, true, false)
                {
                    crate::logging::warn(format!(
                        "Cannot transfer await dependency {id} delivery to headless runner: {error}"
                    ));
                }
                awaits.push(id.clone());
                continue;
            }
            if let Some(task) = manager.status(id).await
                && task.session_id == session_id
                && task.task_id == *id
            {
                tasks.push(task);
            }
        }
        let actionable = run_actionable_todos(todos, &tasks, session_id);
        let waiting: Vec<_> = tasks
            .iter()
            .filter(|task| run_task_is_waitable(task, session_id))
            .collect();
        for task in &waiting {
            // Prevent a second, daemon-owned turn for the same completion. Do this
            // even for mixed work, whose independent todos remain actionable.
            if task.wake {
                if let Err(error) = manager
                    .update_delivery(&task.task_id, task.notify, false)
                    .await
                {
                    crate::logging::warn(format!(
                        "Cannot disable daemon wake for dependency {}: {error}",
                        task.task_id
                    ));
                }
            }
            if task.stall_wake_seconds.is_some() {
                manager.disarm_stall_watchdog(&task.task_id).await;
            }
        }
        let all_waiting = (!waiting.is_empty() || !awaits.is_empty())
            && !actionable.iter().any(crate::todo::is_incomplete_todo);
        if !all_waiting || !may_resume {
            return (
                actionable,
                (!completion_messages.is_empty()).then(|| completion_messages.join("\n\n")),
            );
        }

        let mut waits = futures::stream::FuturesUnordered::new();
        for task in waiting {
            let id = task.task_id.clone();
            waits.push(
                async move {
                    loop {
                        let result = manager
                            .wait(&id, std::time::Duration::from_secs(3600), false)
                            .await;
                        match result {
                            Some(result) if run_task_is_waitable(&result.task, session_id) => {}
                            _ => return id,
                        }
                    }
                }
                .boxed(),
            );
        }
        for id in awaits {
            waits.push(
                async move {
                    while crate::todo::await_dependency::running(
                        &crate::storage::runtime_dir(),
                        &id,
                        session_id,
                    ) {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    id
                }
                .boxed(),
            );
        }
        // Resume when any dependency resolves, not only the first task in the todo
        // list. Other tasks can remain in flight while newly unblocked work runs.
        let Some(resolved) = waits.next().await else {
            crate::logging::warn("Headless dependency wait set unexpectedly empty");
            return (actionable, None);
        };
        let status = manager.status(&resolved).await;
        let mut message = format!(
            "[auto] Background dependency {resolved} resolved. Reassess the waiting todos and continue newly actionable work. Do not retry failed tasks automatically or work on todos whose other dependencies are still running.\n"
        );
        match status.filter(|task| task.session_id == session_id) {
            Some(task) => {
                message.push_str(&format!(
                    "Status: {:?}; exit code: {:?}; error: {:?}\n",
                    task.status, task.exit_code, task.error
                ));
                if let Some(output) = manager.output(&resolved).await {
                    message.push_str("Task output (tail):\n");
                    let tail: String = output.chars().rev().take(16_000).collect();
                    message.extend(tail.chars().rev());
                }
            }
            None => {
                if let Some(state) =
                    crate::todo::await_dependency::load(&crate::storage::runtime_dir(), &resolved)
                {
                    message.push_str(&format!("Swarm await result: {:?}", state.final_response));
                } else {
                    message.push_str("Task status is unavailable. Inspect or repair the stale dependency instead of waiting for it again.");
                }
            }
        }
        completion_messages.push(message);
    }
}

/// Build the deferred quality-check reminder for a headless run, consuming the
/// turn's observation log.
///
/// The log is cleared after a review can be built, so one turn's points cannot
/// be raised again against the next turn's work. Failed context loads are logged
/// and leave the observations intact for a later attempt.
pub(super) fn take_run_gate_digest(session_id: &str, already_delivered: bool) -> Option<String> {
    if already_delivered {
        return None;
    }
    let observations = match crate::todo::load_gate_observations(session_id) {
        Ok(observations) => observations,
        Err(error) => {
            crate::logging::warn(format!(
                "Cannot load quality observations for {session_id}: {error}"
            ));
            return None;
        }
    };
    if observations.is_empty() {
        return None;
    }
    let plan = match crate::todo::load_plan(session_id) {
        Ok(plan) => plan,
        Err(error) => {
            crate::logging::warn(format!(
                "Cannot load quality review plan for {session_id}: {error}"
            ));
            return None;
        }
    };
    let goals = match crate::todo::load_goals(session_id) {
        Ok(goals) => goals,
        Err(error) => {
            crate::logging::warn(format!(
                "Cannot load quality review goals for {session_id}: {error}"
            ));
            return None;
        }
    };
    let digest = crate::todo::build_gate_digest(&observations, &plan, &goals);
    if let Err(error) = crate::todo::clear_gate_observations(session_id) {
        crate::logging::warn(format!(
            "Cannot clear quality observations for {session_id}: {error}"
        ));
    }
    digest
}

/// Consume the observation log only once the turn has actually ended.
///
/// `take_run_gate_digest` clears the log, so calling it while todos are still
/// open would destroy the reminder: auto-poke iterates many times with open work
/// on a long run, and the incomplete-todo follow-up takes precedence, so the
/// digest string would be dropped on the floor with the log already emptied.
pub(super) fn take_run_gate_digest_if_turn_ended(
    session_id: &str,
    already_delivered: bool,
    todos: &[crate::todo::TodoItem],
    enabled: bool,
) -> Option<String> {
    if !enabled {
        if let Err(error) = crate::todo::clear_gate_observations(session_id) {
            crate::logging::warn(format!(
                "Cannot clear quality observations for {session_id}: {error}"
            ));
        }
        return None;
    }
    let work_remains = todos.iter().any(|todo| {
        !crate::todo::todo_status_is_completed(&todo.status)
            && !crate::todo::todo_status_is_cancelled(&todo.status)
    });
    if work_remains {
        return None;
    }
    take_run_gate_digest(session_id, already_delivered)
}

pub(super) fn build_run_auto_poke_follow_up_from_todos(
    todos: &[crate::todo::TodoItem],
    confidence_spike_challenged: bool,
    gate_digest: Option<String>,
) -> Option<RunAutoPokeFollowUp> {
    let incomplete: Vec<_> = todos
        .iter()
        .filter(|todo| {
            !crate::todo::todo_status_is_completed(&todo.status)
                && !crate::todo::todo_status_is_cancelled(&todo.status)
        })
        .cloned()
        .collect();
    if !incomplete.is_empty() {
        return Some(RunAutoPokeFollowUp::Incomplete {
            count: incomplete.len(),
            message: build_run_poke_message(&incomplete),
        });
    }
    // Verify the weak points before judging completion confidence: the digest
    // may prompt work that changes those very assessments.
    if let Some(message) = gate_digest {
        return Some(RunAutoPokeFollowUp::GateDigest { message });
    }
    if !todos.is_empty()
        && let Some((message, confidence_spike_challenge)) =
            build_run_todo_validation_message(todos, !confidence_spike_challenged)
    {
        return Some(RunAutoPokeFollowUp::ConfidenceSummary {
            total_todos: todos.len(),
            message,
            confidence_spike_challenge,
        });
    }
    None
}

pub(super) fn build_run_poke_message(incomplete: &[crate::todo::TodoItem]) -> String {
    crate::todo::build_auto_poke_message(incomplete.len())
}

pub(super) fn build_run_todo_validation_message(
    todos: &[crate::todo::TodoItem],
    allow_confidence_spike_challenge: bool,
) -> Option<(String, bool)> {
    let completed: Vec<&crate::todo::TodoItem> = todos
        .iter()
        .filter(|todo| crate::todo::todo_status_is_completed(&todo.status))
        .collect();
    if completed.is_empty() {
        return None;
    }

    let completion_confidence_needs_validation = completed
        .iter()
        .any(|todo| !crate::todo::completion_confidence_passes(todo.completion_confidence));
    let confidence_spike_detected =
        allow_confidence_spike_challenge && !crate::todo::spike_completed_todos(todos).is_empty();

    if !completion_confidence_needs_validation && !confidence_spike_detected {
        // Nothing actionable: completing the loop with a generic summary just
        // spends tokens on "all good" theater, so send nothing and end the run.
        return None;
    }

    if completion_confidence_needs_validation {
        crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::Completion);
        Some((
            crate::todo::build_todo_completion_continuation_message(todos),
            false,
        ))
    } else {
        crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::ConfidenceSpike);
        Some((
            crate::todo::build_todo_confidence_spike_continuation_message(todos),
            true,
        ))
    }
}
