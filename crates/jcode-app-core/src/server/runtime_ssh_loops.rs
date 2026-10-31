//! Server-owned remote loop control. Only sessions explicitly opted in through
//! remote.runtime_command get a state file, so ordinary local clients retain
//! their existing client-side follow-up behavior.
use crate::{agent::Agent, protocol::ServerEvent, todo};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tokio::sync::mpsc;

const MAX_CONTINUATIONS_PER_TURN: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Workflow {
    kind: String,
    plan_only: bool,
    focus: Option<String>,
    active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct LoopState {
    version: u32,
    poke_enabled: bool,
    workflow: Option<Workflow>,
    policy: todo::followup::FollowUpPolicy,
    turns: u64,
    last_notice: Option<String>,
}

impl Default for LoopState {
    fn default() -> Self {
        Self {
            version: 1,
            poke_enabled: false,
            workflow: None,
            policy: todo::followup::FollowUpPolicy::new(false),
            turns: 0,
            last_notice: None,
        }
    }
}

fn state_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn state_path_at(root: &Path, session_id: &str) -> Result<PathBuf> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("Invalid runtime session ID");
    }
    Ok(root.join("remote-loops").join(format!("{session_id}.json")))
}

fn state_path(session_id: &str) -> Result<PathBuf> {
    state_path_at(&crate::storage::jcode_dir()?, session_id)
}

fn read_state(path: &Path) -> Result<LoopState> {
    if !path.exists() {
        return Ok(LoopState::default());
    }
    let state: LoopState =
        crate::storage::read_json(path).context("Cannot read persisted remote loop state")?;
    if state.version != 1 {
        bail!("Unsupported remote loop state version {}", state.version);
    }
    Ok(state)
}

fn update_state<T>(
    session_id: &str,
    action: impl FnOnce(&mut LoopState) -> Result<T>,
) -> Result<T> {
    let _lock = state_lock().lock().unwrap_or_else(|p| p.into_inner());
    let path = state_path(session_id)?;
    let mut state = read_state(&path)?;
    let value = action(&mut state)?;
    crate::storage::write_json(&path, &state)?;
    Ok(value)
}

pub(super) fn handles(command: &str) -> bool {
    matches!(command, "poke" | "improve" | "refactor" | "fix")
}

fn status_data(session_id: &str, state: &LoopState) -> Result<Value> {
    let todos = todo::load_todos(session_id)?;
    Ok(
        json!({"session_id": session_id, "poke_enabled": state.poke_enabled,
        "workflow": state.workflow, "turns": state.turns, "last_notice": state.last_notice,
        "todos": todos, "server_owned": true}),
    )
}

fn status_result(session_id: &str, state: &LoopState) -> Result<(Value, String)> {
    let data = status_data(session_id, state)?;
    Ok((data.clone(), serde_json::to_string_pretty(&data)?))
}

/// Status and stop do not need the Agent mutex. A user can disarm a loop while
/// its current model/tool turn is running. Stopping never cancels unrelated work.
pub(super) fn control(
    session_id: &str,
    command: &str,
    args: &[String],
) -> Result<Option<(Value, String)>> {
    if !matches!(command, "poke" | "improve" | "refactor") {
        return Ok(None);
    }
    let [action] = args else {
        return Ok(None);
    };
    if action == "status" {
        let _lock = state_lock().lock().unwrap_or_else(|p| p.into_inner());
        return status_result(session_id, &read_state(&state_path(session_id)?)?).map(Some);
    }
    if !matches!(action.as_str(), "off" | "stop") {
        return Ok(None);
    }
    update_state(session_id, |state| {
        if command == "poke" {
            state.poke_enabled = false;
            if let Some(workflow) = state.workflow.as_mut() {
                workflow.active = false;
            }
        } else if let Some(workflow) = state.workflow.as_mut().filter(|w| w.kind == command) {
            workflow.active = false;
        } else {
            return status_result(session_id, state).map(Some);
        }
        // Workflow stop disarms its own continuation. Explicit poke mode can
        // still continue independent todos only when no workflow owns them.
        state.policy.disable();
        state.last_notice = Some(format!(
            "Stopped /{command} at the next turn boundary. The current turn is not cancelled."
        ));
        let data = status_data(session_id, state)?;
        Ok(Some((data, state.last_notice.clone().unwrap_or_default())))
    })
}

fn launch_prompt(workflow: &Workflow, resume: bool) -> String {
    let aim = if workflow.kind == "refactor" {
        "behavior-preserving refactors, module boundaries, dead code and test isolation"
    } else {
        "high-leverage reliability, UX, tests and measurable performance improvements"
    };
    let focus = workflow
        .focus
        .as_deref()
        .unwrap_or("the repository's highest-leverage safe opportunities");
    let execution = if workflow.plan_only {
        "Plan only. Do not edit files, apply patches, or change git state. Stop after a ranked plan and rationale."
    } else {
        "Implement the strongest safe items and validate meaningful changes. Reassess after each batch. Stop at diminishing returns, do not invent busywork or perform speculative rewrites."
    };
    let review = if workflow.kind == "refactor" && !workflow.plan_only {
        " After a meaningful batch, get one independent read-only review, explicitly forbidding edits or git changes, then address valid findings."
    } else {
        ""
    };
    format!(
        "{} server-owned {} workflow in this session. Inspect the current code and git state. Focus: {focus}. Prioritize {aim}. Read existing todos before updating them into a ranked, scoped plan of 3-7 worthwhile items. {execution}{review} Preserve unrelated work and accurately maintain the todo state.",
        if resume {
            "Resume the persisted"
        } else {
            "Start a"
        },
        workflow.kind
    )
}

/// Structured execution, never arbitrary slash-prompt fallthrough. next_prompt
/// is a server-generated work item that the runtime dispatcher launches on the
/// remote Agent using its normal turn lifecycle after dropping the Agent lock.
pub(super) async fn execute(
    agent: &mut Agent,
    command: &str,
    args: &[String],
) -> Result<(Value, String)> {
    let session_id = agent.session_id().to_string();
    if let Some(result) = control(&session_id, command, args)? {
        return Ok(result);
    }
    if command == "fix" {
        if !args.is_empty() {
            bail!("Usage: /fix");
        }
        let data = agent.recover_runtime_context()?;
        let message = format!(
            "Recovered remote runtime context in session `{session_id}` without clearing history. Repaired {} missing tool outputs and reset provider resume/cache state. {}",
            data["repaired_tool_outputs"],
            data["compaction_message"].as_str().unwrap_or_default()
        );
        return Ok((data, message));
    }
    if command == "poke" {
        if !args.is_empty()
            && args != ["on".to_string()]
            && args != ["continue".to_string()]
            && args != ["resume".to_string()]
        {
            bail!("Usage: /poke [on|off|status|continue]");
        }
        return update_state(&session_id, |state| {
            state.poke_enabled = true;
            // /poke is an explicit restart, not an implicit budget reset after
            // every model turn. Circuit breakers survive ordinary turns/reload.
            state.policy = todo::followup::FollowUpPolicy::new(true);
            state.last_notice = None;
            if let Some(workflow) = state.workflow.as_mut() {
                workflow.active = true;
            }
            let snapshot = snapshot(&session_id)?;
            let decision = state.policy.decide(&snapshot);
            apply_effects(&session_id, &decision.effects)?;
            let next_prompt = decision.follow_up.as_ref().map(|f| f.message.clone());
            if let Some(followup) = decision.follow_up {
                state.last_notice = Some(followup.notice);
            }
            let mut data = status_data(&session_id, state)?;
            if let Some(prompt) = next_prompt {
                data["next_prompt"] = json!(prompt);
            }
            let message = state.last_notice.clone().unwrap_or_else(|| {
                "Remote poke enabled. No actionable todo continuation is needed.".into()
            });
            Ok((data, message))
        });
    }
    if !matches!(command, "improve" | "refactor") {
        bail!("Unsupported remote loop command: {command}");
    }
    let resume = args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "resume" | "continue"));
    if args.len() > 1
        && args.first().is_some_and(|a| {
            matches!(
                a.as_str(),
                "status" | "stop" | "off" | "resume" | "continue"
            )
        })
    {
        bail!("Usage: /{command} [focus|plan [focus]|resume|status|stop]");
    }
    update_state(&session_id, |state| {
        if resume {
            let workflow = state
                .workflow
                .as_mut()
                .filter(|w| w.kind == command)
                .context("No persisted workflow of this kind to resume. Start one first.")?;
            workflow.active = true;
        } else {
            let plan_only = args.first().is_some_and(|a| a == "plan");
            let focus_args = if plan_only { &args[1..] } else { args };
            let focus = (!focus_args.is_empty()).then(|| focus_args.join(" "));
            state.workflow = Some(Workflow {
                kind: command.into(),
                plan_only,
                focus,
                active: true,
            });
        }
        let workflow = state.workflow.as_ref().expect("workflow initialized");
        state.policy = todo::followup::FollowUpPolicy::new(!workflow.plan_only);
        let prompt = launch_prompt(workflow, resume);
        state.last_notice = Some(format!(
            "{} remote /{command}{} workflow.",
            if resume { "Resumed" } else { "Started" },
            if workflow.plan_only { " planning" } else { "" }
        ));
        let mut data = status_data(&session_id, state)?;
        data["next_prompt"] = json!(prompt);
        Ok((data, state.last_notice.clone().unwrap_or_default()))
    })
}

fn snapshot(session_id: &str) -> Result<todo::followup::TodoSnapshot> {
    Ok(todo::store::load_snapshot(
        &crate::storage::jcode_dir()?,
        session_id,
    ))
}

fn apply_effects(session_id: &str, effects: &todo::followup::TodoEffects) -> Result<()> {
    if effects.clear_gate_observations {
        todo::clear_gate_observations(session_id)?;
    }
    if effects.mark_long_session_review_delivered {
        todo::take_long_session_review_if_due(session_id)?;
    }
    Ok(())
}

fn next_followup(
    state: &mut LoopState,
    snapshot: &todo::followup::TodoSnapshot,
) -> todo::followup::Decision {
    if let Some(workflow) = &state.workflow {
        if !workflow.active || workflow.plan_only {
            return Default::default();
        }
    } else if !state.poke_enabled {
        return Default::default();
    }
    let decision = state.policy.decide_after_turn(snapshot, true);
    state.turns += u64::from(decision.follow_up.is_some());
    state.last_notice = decision
        .follow_up
        .as_ref()
        .map(|f| f.notice.clone())
        .or_else(|| decision.notice.clone());
    decision
}

/// Run inside the server turn task before releasing its processing guard. Never
/// recursively invokes turn_failover, and rereads durable state each turn so
/// stop/status work from another connection while a model/tool is busy.
pub(super) async fn run_followups(
    agent: &mut Agent,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let session_id = agent.session_id().to_string();
    if !state_path(&session_id)?.exists() {
        return Ok(());
    }
    for _ in 0..MAX_CONTINUATIONS_PER_TURN {
        if agent.graceful_shutdown_signal().is_set() {
            return Ok(());
        }
        let mut snapshot = snapshot(&session_id)?;
        snapshot.running_background_tasks = crate::background::global()
            .list()
            .await
            .into_iter()
            .filter(|task| {
                task.session_id == session_id
                    && matches!(task.status, crate::bus::BackgroundTaskStatus::Running)
            })
            .map(|task| task.task_id)
            .collect();
        let decision = update_state(&session_id, |state| Ok(next_followup(state, &snapshot)))?;
        apply_effects(&session_id, &decision.effects)?;
        let Some(followup) = decision.follow_up else {
            return Ok(());
        };
        // Persisted budgets are committed before the request. A daemon restart
        // cannot turn a failing continuation into an unlimited retry loop.
        let result = super::client_lifecycle::process_locked_message_streaming_mpsc(
            agent,
            &followup.message,
            Vec::new(),
            None,
            event_tx.clone(),
        )
        .await;
        let result = super::turn_failover::retry_turn_with_server_failover(
            agent,
            result,
            None,
            event_tx.clone(),
        )
        .await;
        if let Err(error) = &result {
            update_state(&session_id, |state| {
                state.policy.disable();
                state.poke_enabled = false;
                if let Some(workflow) = state.workflow.as_mut() {
                    workflow.active = false;
                }
                state.last_notice = Some(format!(
                    "Paused after runtime error: {error}. Use /poke or /improve resume after recovery."
                ));
                Ok(())
            })?;
        }
        result?;
    }
    update_state(&session_id, |state| {
        state.policy.disable();
        state.poke_enabled = false;
        if let Some(workflow) = state.workflow.as_mut() {
            workflow.active = false;
        }
        state.last_notice = Some(
            "Paused at the server continuation safety limit. Inspect status before resuming."
                .into(),
        );
        Ok(())
    })
}

#[cfg(test)]
#[path = "runtime_ssh_loops_tests.rs"]
mod tests;
