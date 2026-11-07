//! Mirrors Claude Code background work (subagents, shells, monitors) into
//! jcode's background-task rows, so the TUI shows what is still running.
//!
//! Claude Code sends the full running set in `system/background_tasks_changed`
//! whenever it changes, including between turns when no jcode turn is
//! listening. Diffing those snapshots needs no start/finish pairing, and the
//! preceding `system/task_notification` supplies each task's outcome.

use jcode_base::bus::{BackgroundTaskCompleted, BackgroundTaskStatus, Bus, BusEvent, UiActivity};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// jcode session the provider serves; set by `Provider::bind_session`.
pub type SessionSlot = Arc<Mutex<Option<String>>>;

struct Running {
    kind: String,
    display: String,
    started: Instant,
}

struct Outcome {
    failed: bool,
    summary: String,
    output_file: String,
}

pub struct BackgroundTasks {
    session: SessionSlot,
    running: Mutex<BTreeMap<String, Running>>,
    outcomes: Mutex<HashMap<String, Outcome>>,
}

/// Claude Code's own labels for task types.
fn kind_label(task_type: &str) -> &str {
    match task_type {
        "local_agent" => "subagent",
        "local_bash" => "shell",
        "local_workflow" => "workflow",
        "monitor_mcp" | "monitor_ws" => "monitor",
        "mcp_task" => "MCP task",
        "in_process_teammate" => "teammate",
        "remote_agent" => "cloud session",
        other => other,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl BackgroundTasks {
    pub fn new(session: SessionSlot) -> Self {
        Self {
            session,
            running: Mutex::default(),
            outcomes: Mutex::default(),
        }
    }

    /// Feed every `system` line from the child, in or out of a turn.
    pub fn observe(&self, line: &Value) {
        let str_of = |value: &Value, key: &str| value[key].as_str().unwrap_or("").to_string();
        match line["subtype"].as_str() {
            Some("task_notification") => {
                lock(&self.outcomes).insert(
                    str_of(line, "task_id"),
                    Outcome {
                        failed: !matches!(line["status"].as_str(), Some("completed") | None),
                        summary: str_of(line, "summary"),
                        output_file: str_of(line, "output_file"),
                    },
                );
            }
            Some("background_tasks_changed") => {
                let current: BTreeMap<String, (String, String)> = line["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|task| {
                        let kind = kind_label(task["task_type"].as_str().unwrap_or("task"));
                        let description = str_of(task, "description");
                        let display = if description.is_empty() {
                            kind.to_string()
                        } else {
                            format!("{kind}: {description}")
                        };
                        (str_of(task, "task_id"), (kind.to_string(), display))
                    })
                    .filter(|(id, _)| !id.is_empty())
                    .collect();
                let mut running = lock(&self.running);
                let finished: Vec<String> = running
                    .keys()
                    .filter(|id| !current.contains_key(*id))
                    .cloned()
                    .collect();
                for id in finished {
                    if let Some(task) = running.remove(&id) {
                        let outcome = lock(&self.outcomes).remove(&id);
                        self.publish_finished(&id, task, outcome);
                    }
                }
                for (id, (kind, display)) in current {
                    if running.contains_key(&id) {
                        continue;
                    }
                    self.publish_started(&id, &kind, &display);
                    running.insert(
                        id,
                        Running {
                            kind,
                            display,
                            started: Instant::now(),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn session(&self) -> Option<String> {
        lock(&self.session).clone()
    }

    fn publish_started(&self, id: &str, kind: &str, display: &str) {
        let Some(session_id) = self.session() else {
            return;
        };
        let label = jcode_base::message::background_task_display_label(kind, Some(display))
            .replace('`', "'");
        Bus::global().publish(BusEvent::UiActivity(UiActivity::background(
            Some(session_id),
            format!("**Background task started** `{id}` · `{label}`"),
            Some(format!("Background {label}")),
        )));
    }

    fn publish_finished(&self, id: &str, task: Running, outcome: Option<Outcome>) {
        let Some(session_id) = self.session() else {
            return;
        };
        let failed = outcome.as_ref().is_some_and(|o| o.failed);
        Bus::global().publish(BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
            task_id: id.to_string(),
            tool_name: task.kind,
            display_name: Some(task.display),
            session_id,
            status: if failed {
                BackgroundTaskStatus::Failed
            } else {
                BackgroundTaskStatus::Completed
            },
            exit_code: None,
            output_preview: outcome
                .as_ref()
                .map_or_else(String::new, |o| o.summary.clone()),
            output_file: outcome.map_or_else(Default::default, |o| o.output_file.into()),
            duration_secs: task.started.elapsed().as_secs_f64(),
            notify: true,
            // Claude Code delivers the result to its own model; jcode only
            // updates the row.
            wake: false,
        }));
    }
}

impl Drop for BackgroundTasks {
    /// A replaced or exited child takes its tasks with it; close their rows.
    fn drop(&mut self) {
        let running = std::mem::take(&mut *lock(&self.running));
        for (id, task) in running {
            self.publish_finished(
                &id,
                task,
                Some(Outcome {
                    failed: true,
                    summary: "Claude Code process ended".to_string(),
                    output_file: String::new(),
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(tasks: Value) -> Value {
        json!({"type": "system", "subtype": "background_tasks_changed", "tasks": tasks})
    }

    #[test]
    fn snapshots_publish_start_and_finish_with_outcome() {
        let session: SessionSlot = Arc::new(Mutex::new(Some("session-bg-test".into())));
        let mut rx = Bus::global().subscribe();
        let tasks = BackgroundTasks::new(session);

        tasks.observe(&snapshot(json!([
            {"task_id": "b1", "task_type": "local_bash", "description": "sleep probe"}
        ])));
        // Unchanged snapshot: no duplicate start.
        tasks.observe(&snapshot(json!([
            {"task_id": "b1", "task_type": "local_bash", "description": "sleep probe"}
        ])));
        tasks.observe(&json!({"type": "system", "subtype": "task_notification",
            "task_id": "b1", "status": "failed", "summary": "exit 1",
            "output_file": "/tmp/b1.output"}));
        tasks.observe(&snapshot(json!([])));

        let mut started = Vec::new();
        let mut finished = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                BusEvent::UiActivity(a) if a.session_id.as_deref() == Some("session-bg-test") => {
                    started.push(a.message)
                }
                BusEvent::BackgroundTaskCompleted(c) if c.session_id == "session-bg-test" => {
                    finished.push(c)
                }
                _ => {}
            }
        }
        assert_eq!(
            started,
            ["**Background task started** `b1` · `shell: sleep probe`"]
        );
        let parsed =
            jcode_base::message::parse_background_task_started_notification_markdown(&started[0])
                .expect("TUI parses the start row");
        assert_eq!(parsed.label, "shell: sleep probe");
        assert_eq!(finished.len(), 1);
        assert!(matches!(finished[0].status, BackgroundTaskStatus::Failed));
        assert!(!finished[0].wake);
        // The TUI finishes the same row from the completion markdown.
        let markdown =
            jcode_base::message::format_background_task_notification_markdown(&finished[0]);
        let completed = jcode_base::message::parse_background_task_notification_markdown(&markdown)
            .expect("TUI parses the completion");
        assert_eq!(completed.task_id, "b1");
        assert_eq!(
            jcode_base::message::background_task_display_label(
                &completed.tool_name,
                completed.display_name.as_deref()
            ),
            "shell: sleep probe"
        );
    }
}
