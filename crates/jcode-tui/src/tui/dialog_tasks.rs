//! Claude Code's background tasks dialog (`/tasks`, or ↓ on the footer pill):
//! running shells and spawned agents, recently finished ones, a detail page
//! per task with its output tail, and `x` to stop.

use super::*;
use crate::protocol::SwarmMemberStatus;
use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};

/// Finished shells and agents listed under `Completed`.
const COMPLETED_CAP: usize = 10;
/// Output lines a detail page shows, like Claude Code's.
pub const TAIL_LINES: usize = 10;

/// One background shell as the dialog shows it.
#[derive(Debug, Clone)]
pub struct ShellTask {
    pub task_id: String,
    pub label: String,
    pub status: BackgroundTaskRowStatus,
    pub runtime_secs: Option<u64>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// `None` when the output file is not on this machine (a remote daemon).
    pub output: Option<Vec<String>>,
}

/// Everything the dialog lists, refreshed by the app about once a second.
#[derive(Debug, Clone, Default)]
pub struct TasksSnapshot {
    pub shells: Vec<ShellTask>,
    pub agents: Vec<SwarmMemberStatus>,
    pub taken_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskTarget {
    Shell(String),
    Agent(String),
}

/// An agent still working, as opposed to finished or idle.
pub fn agent_is_live(status: &str) -> bool {
    !matches!(
        status,
        "completed"
            | "done"
            | "ready"
            | "failed"
            | "crashed"
            | "stopped"
            | "closed"
            | "disconnected"
    )
}

/// Claude Code's footer pill: `1 shell`, `N shells`, `N background agents`,
/// or `N background tasks` when both kinds run.
pub fn pill_text(shells: usize, agents: usize) -> Option<String> {
    let count = |n: usize, noun: &str| format!("{n} {noun}{}", if n == 1 { "" } else { "s" });
    match (shells, agents) {
        (0, 0) => None,
        (shells, 0) => Some(count(shells, "shell")),
        (0, agents) => Some(count(agents, "background agent")),
        (shells, agents) => Some(count(shells + agents, "background task")),
    }
}

/// The pill for this session's running shells and live spawned agents.
pub fn task_pill(rows: &[BackgroundTaskRow], agents: &[SwarmMemberStatus]) -> Option<String> {
    pill_text(
        rows.iter()
            .filter(|row| row.status == BackgroundTaskRowStatus::Running)
            .count(),
        agents
            .iter()
            .filter(|agent| agent_is_live(&agent.status))
            .count(),
    )
}

/// The last `max` non-blank lines of a task's output file, with terminal
/// escapes removed. Reads only the end of the file; `None` if it is absent.
pub fn read_output_tail(path: &std::path::Path, max: usize) -> Option<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL_BYTES: u64 = 16 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut bytes = Vec::new();
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .is_err()
        || file.read_to_end(&mut bytes).is_err()
    {
        return Some(Vec::new());
    }
    let text = crate::message::strip_ansi_escape_sequences(&String::from_utf8_lossy(&bytes));
    let lines: Vec<String> = text
        .lines()
        // A progress bar redraws with \r: keep what the terminal would show.
        .map(|line| line.rsplit('\r').next().unwrap_or(line))
        .map(|line| line.replace('\t', "    "))
        .filter(|line| !line.trim().is_empty())
        .collect();
    Some(lines[lines.len().saturating_sub(max)..].to_vec())
}

fn agent_label(agent: &SwarmMemberStatus) -> String {
    let name = agent
        .friendly_name
        .clone()
        .unwrap_or_else(|| agent.session_id.chars().take(8).collect());
    match agent.task_label.as_deref() {
        Some(task) if !task.is_empty() && task != name => format!("{name}: {task}"),
        _ => name,
    }
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn elapsed(secs: Option<u64>) -> String {
    secs.map(crate::kv_cache_monitor::format_duration_secs)
        .unwrap_or_default()
}

fn shell_tone(status: BackgroundTaskRowStatus) -> Tone {
    match status {
        BackgroundTaskRowStatus::Running => Tone::Running,
        BackgroundTaskRowStatus::Completed => Tone::Success,
        BackgroundTaskRowStatus::Failed => Tone::Error,
    }
}

fn agent_tone(status: &str) -> Tone {
    match status {
        "completed" | "done" | "ready" => Tone::Success,
        "failed" | "crashed" => Tone::Error,
        "stopped" | "closed" | "disconnected" => Tone::Warning,
        _ => Tone::Running,
    }
}

impl Dialog {
    pub fn tasks(snapshot: TasksSnapshot) -> Self {
        let mut dialog = Self {
            stack: Vec::new(),
            mcp: None,
            skills: None,
            plugin: None,
            rewind: None,
            ask: None,
            tasks: Some(snapshot),
            pending_rewind: false,
            status: None,
            pending_mcp: None,
        };
        dialog.stack.push(dialog.tasks_list_view());
        dialog
    }

    pub fn is_tasks(&self) -> bool {
        self.tasks.is_some()
    }

    /// Claude Code re-polls a task's output every second while it is shown.
    pub fn tasks_refresh_due(&self) -> bool {
        self.tasks.as_ref().is_some_and(|tasks| {
            tasks
                .taken_at
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(1))
        })
    }

    /// New data: rebuild the open views, keeping their selection. A detail
    /// page whose task disappeared closes.
    pub fn set_tasks(&mut self, snapshot: TasksSnapshot) {
        if self.tasks.is_none() {
            return;
        }
        self.tasks = Some(snapshot);
        self.status = None;
        self.rebuild();
        if self.stack.is_empty() {
            self.stack.push(self.tasks_list_view());
        }
    }

    /// `x` stops the selected or shown task; Enter on an agent page opens it.
    pub(super) fn tasks_key(&mut self, code: KeyCode) -> Option<DialogKey> {
        let view = self.stack.last()?;
        let target = match &view.route {
            Route::TaskShell(id) => TaskTarget::Shell(id.clone()),
            Route::TaskAgent(id) => TaskTarget::Agent(id.clone()),
            Route::Tasks => match view
                .visible()
                .get(view.selected)
                .and_then(|&index| view.items[index].action.as_ref())
            {
                Some(ItemAction::Open(Route::TaskShell(id))) => TaskTarget::Shell(id.clone()),
                Some(ItemAction::Open(Route::TaskAgent(id))) => TaskTarget::Agent(id.clone()),
                _ => return None,
            },
            _ => return None,
        };
        match (code, &target) {
            (KeyCode::Char('x'), _) if self.task_running(&target) => {
                self.status = Some("Stopping…".to_string());
                Some(DialogKey::StopTask(target))
            }
            (KeyCode::Enter, TaskTarget::Agent(id))
                if matches!(view.route, Route::TaskAgent(_)) =>
            {
                Some(DialogKey::EnterAgent(id.clone()))
            }
            _ => None,
        }
    }

    fn task_running(&self, target: &TaskTarget) -> bool {
        let Some(tasks) = &self.tasks else {
            return false;
        };
        match target {
            TaskTarget::Shell(id) => tasks.shells.iter().any(|shell| {
                shell.task_id == *id && shell.status == BackgroundTaskRowStatus::Running
            }),
            TaskTarget::Agent(id) => tasks
                .agents
                .iter()
                .any(|agent| agent.session_id == *id && agent_is_live(&agent.status)),
        }
    }

    pub(super) fn tasks_list_view(&self) -> View {
        let mut view = View::new(Route::Tasks, "Background tasks");
        view.empty = "No tasks currently running".to_string();
        let Some(tasks) = &self.tasks else {
            return view;
        };
        let shell_running = |shell: &&ShellTask| shell.status == BackgroundTaskRowStatus::Running;
        view.subtitle = pill_text(
            tasks.shells.iter().filter(shell_running).count(),
            tasks
                .agents
                .iter()
                .filter(|agent| agent_is_live(&agent.status))
                .count(),
        )
        .unwrap_or_default();
        let shell_item = |group: &str, shell: &ShellTask| Item {
            group: Some(group.to_string()),
            icon: Some(shell_tone(shell.status)),
            label: single_line(&shell.label),
            meta: vec![elapsed(shell.runtime_secs)],
            search_text: String::new(),
            action: Some(ItemAction::Open(Route::TaskShell(shell.task_id.clone()))),
        };
        let agent_item = |group: &str, agent: &SwarmMemberStatus| Item {
            group: Some(group.to_string()),
            icon: Some(agent_tone(&agent.status)),
            label: single_line(&agent_label(agent)),
            meta: vec![elapsed(agent.runtime.elapsed_secs)],
            search_text: String::new(),
            action: Some(ItemAction::Open(Route::TaskAgent(agent.session_id.clone()))),
        };
        // Newest first within each section.
        for shell in tasks.shells.iter().rev().filter(shell_running) {
            view.items.push(shell_item("Shells", shell));
        }
        for agent in tasks.agents.iter().filter(|a| agent_is_live(&a.status)) {
            view.items.push(agent_item("Agents", agent));
        }
        let finished_shells = tasks.shells.iter().rev().filter(|s| !shell_running(s));
        let finished_agents = tasks.agents.iter().filter(|a| !agent_is_live(&a.status));
        let completed: Vec<Item> = finished_shells
            .map(|shell| shell_item("Completed", shell))
            .chain(finished_agents.map(|agent| agent_item("Completed", agent)))
            .take(COMPLETED_CAP)
            .collect();
        view.items.extend(completed);
        view
    }

    pub(super) fn task_shell_view(&self, id: &str) -> View {
        let route = Route::TaskShell(id.to_string());
        let Some(shell) = self
            .tasks
            .iter()
            .flat_map(|tasks| &tasks.shells)
            .find(|shell| shell.task_id == id)
        else {
            return View::new(route, "");
        };
        let mut view = View::new(route, single_line(&shell.label));
        view.subtitle = "shell".to_string();
        let status = match shell.status {
            BackgroundTaskRowStatus::Running => "running".to_string(),
            BackgroundTaskRowStatus::Completed => match shell.exit_code {
                Some(code) => format!("✔ done · exit code {code}"),
                None => "✔ done".to_string(),
            },
            BackgroundTaskRowStatus::Failed => match (&shell.error, shell.exit_code) {
                (Some(error), _) => format!("✘ failed · {}", single_line(error)),
                (None, Some(code)) => format!("✘ failed · exit code {code}"),
                (None, None) => "✘ failed".to_string(),
            },
        };
        view.info.push(("Status".to_string(), status));
        if shell.runtime_secs.is_some() {
            view.info
                .push(("Runtime".to_string(), elapsed(shell.runtime_secs)));
        }
        view.info
            .push(("Command".to_string(), single_line(&shell.label)));
        view.boxed = Some((
            "Output".to_string(),
            shell.output.clone().unwrap_or_default(),
            if shell.output.is_none() {
                "Output not available from this client"
            } else if shell.status == BackgroundTaskRowStatus::Running {
                "No output yet"
            } else {
                "No output"
            }
            .to_string(),
        ));
        view
    }

    pub(super) fn task_agent_view(&self, id: &str) -> View {
        let route = Route::TaskAgent(id.to_string());
        let Some(agent) = self
            .tasks
            .iter()
            .flat_map(|tasks| &tasks.agents)
            .find(|agent| agent.session_id == id)
        else {
            return View::new(route, "");
        };
        let mut view = View::new(route, single_line(&agent_label(agent)));
        view.subtitle = "agent".to_string();
        let tone = agent_tone(&agent.status);
        let status = match tone {
            Tone::Running => agent.status.clone(),
            tone => format!("{} {}", tone.glyph(), agent.status),
        };
        view.info.push(("Status".to_string(), status));
        if agent.runtime.elapsed_secs.is_some() {
            view.info
                .push(("Runtime".to_string(), elapsed(agent.runtime.elapsed_secs)));
        }
        if let Some(model) = &agent.runtime.model {
            view.info.push(("Model".to_string(), model.clone()));
        }
        let live = agent.runtime.live_status.as_ref();
        let activity = live
            .and_then(|live| live.tool_intent.clone().or(live.status_detail.clone()))
            .or_else(|| live.and_then(|live| live.tool_name.clone()))
            .or_else(|| agent.detail.clone())
            .or_else(|| agent.runtime.wait_reason.clone());
        if let Some(activity) = activity {
            view.info
                .push(("Activity".to_string(), single_line(&activity)));
        }
        let output: Vec<String> = agent
            .output_tail
            .as_deref()
            .unwrap_or("")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.replace('\t', "    "))
            .collect();
        view.boxed = Some((
            "Output".to_string(),
            output[output.len().saturating_sub(TAIL_LINES)..].to_vec(),
            "No streamed output".to_string(),
        ));
        view
    }
}
