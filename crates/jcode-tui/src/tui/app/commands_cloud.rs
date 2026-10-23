//! `/cloud` and `/local`: move the current session to a cloud host and back.
//!
//! The heavy lifting is the `jcode cloud move|return` CLI. The TUI only runs it
//! for the current session between turns, surfaces the result in the chat, and
//! then reattaches at the new location (SSH attach for `/cloud`, local resume
//! for `/local`). A failure leaves everything where it was.
//!
//! Mid-turn requests replace one pending destination. Only host preparation may
//! run during a turn: it does not snapshot the session or migrate a process.
//! Move/return waits for idle and no staged turn. Status preserves the pending
//! request. Cancellation drops it but does not kill an already-running child.
//! Preparation results belong to the exact request, never its replacement.
//! Unsent composer/queue state refuses acceptance and cancels a pending request
//! at dispatch. A changed active session also cancels it. Canceled host-only
//! preparation does not prevent continued work in the original session.

use super::{App, CloudHandoff, DisplayMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CloudCommand {
    Move { host: Option<String> },
    Return,
    Where,
    Cancel,
    Prepare { host: Option<String> },
}

pub(super) fn parse_cloud_command(trimmed: &str) -> Option<CloudCommand> {
    let mut words = trimmed.split_whitespace();
    match words.next()? {
        "/cloud" => match words.next() {
            None => Some(CloudCommand::Move { host: None }),
            Some("cancel") => Some(CloudCommand::Cancel),
            Some("status") | Some("where") => Some(CloudCommand::Where),
            Some("back") | Some("return") => Some(CloudCommand::Return),
            Some(host) => Some(CloudCommand::Move {
                host: Some(host.to_string()),
            }),
        },
        "/local" => Some(if words.next() == Some("cancel") {
            CloudCommand::Cancel
        } else {
            CloudCommand::Return
        }),
        _ => None,
    }
}

/// Validate before a submit path consumes the composer or its attachments.
pub(super) fn check_cloud_handoff_input(app: &mut App, trimmed: &str) -> bool {
    if !matches!(
        parse_cloud_command(trimmed),
        Some(CloudCommand::Move { .. } | CloudCommand::Return)
    ) {
        return true;
    }
    if app.has_unsent_handoff_work(Some(trimmed)) {
        app.push_display_message(DisplayMessage::error(
            format!("Cloud handoff not queued: {} would be lost. Send or save that work, then retry /cloud or /local. Nothing was removed.", app.unsent_handoff_work(Some(trimmed)).join(", ")),
        ));
        return false;
    }
    true
}

/// Run a `/cloud` family command. Returns true when handled.
pub(super) fn handle_cloud_command(app: &mut App, trimmed: &str, session_id: &str) -> bool {
    let Some(command) = parse_cloud_command(trimmed) else {
        return false;
    };
    if command == CloudCommand::Cancel {
        let message = if app.pending_cloud_handoff.take().is_some() {
            "Pending cloud handoff canceled."
        } else if app.cloud_job.is_some() {
            "A cloud command is already running and cannot be canceled here."
        } else {
            "No pending cloud handoff."
        };
        app.push_display_message(DisplayMessage::system(message));
        return true;
    }
    if app
        .cloud_job
        .as_ref()
        .is_some_and(|job| !matches!(job.command, CloudCommand::Prepare { .. }))
    {
        app.push_display_message(DisplayMessage::error(
            "A cloud command is already in progress.".to_string(),
        ));
        return true;
    }
    if command == CloudCommand::Where && app.pending_cloud_handoff.is_some() {
        let pending = app.pending_cloud_handoff.as_ref().unwrap();
        app.push_display_message(DisplayMessage::system(format!(
            "Pending handoff: {} (session {}). Waiting for an idle turn boundary. Use /cloud cancel to cancel.",
            pending.trimmed, pending.session_id
        )));
        return true;
    }
    if !matches!(command, CloudCommand::Where) {
        if !check_cloud_handoff_input(app, trimmed) {
            return true;
        }
        app.pending_cloud_handoff = Some(PendingCloudHandoff {
            command,
            trimmed: trimmed.to_string(),
            session_id: session_id.to_string(),
            request_id: next_request_id(),
            prepared: false,
            progress: None,
        });
        app.push_display_message(DisplayMessage::system(
            "Cloud handoff queued for the next idle turn boundary. The current turn continues here. Use /cloud cancel to cancel.",
        ));
        // A queued-command dispatcher may still be restoring the composer draft.
        // Only the next tick may prepare or transfer, after that state is visible.
        return true;
    }
    start_cloud_command(app, command, trimmed, session_id, None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct PendingCloudHandoff {
    command: CloudCommand,
    trimmed: String,
    session_id: String,
    request_id: u64,
    prepared: bool,
    /// Checklist started by host preparation, continued by the transfer.
    progress: Option<(usize, Vec<String>)>,
}

fn next_request_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn start_cloud_command(
    app: &mut App,
    command: CloudCommand,
    trimmed: &str,
    session_id: &str,
    carried: Option<(usize, Vec<String>)>,
) -> bool {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            if matches!(command, CloudCommand::Prepare { .. }) {
                app.pending_cloud_handoff = None;
            }
            show_cloud_failure(
                app,
                trimmed,
                session_id,
                None,
                &format!("Cannot locate the jcode binary: {error}"),
            );
            return true;
        }
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--no-update").arg("cloud");
    match &command {
        CloudCommand::Move { host } => {
            cmd.args(["move", "--json", "--session", session_id]);
            if let Some(host) = host {
                cmd.args(["--host", host]);
            }
        }
        CloudCommand::Return => {
            cmd.args(["return", "--json", "--session", session_id]);
        }
        CloudCommand::Prepare { host } => {
            cmd.args(["prepare", "--json"]);
            if let Some(host) = host {
                cmd.args(["--host", host]);
            }
        }
        CloudCommand::Cancel => unreachable!("cancel does not spawn"),
        CloudCommand::Where => {
            cmd.args(["where", "--session", session_id]);
        }
    }
    if app.cloud_job.is_some() {
        app.push_display_message(DisplayMessage::error(
            "A cloud move is already in progress.".to_string(),
        ));
        return true;
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            if matches!(command, CloudCommand::Prepare { .. }) {
                app.pending_cloud_handoff = None;
            }
            show_cloud_failure(
                app,
                trimmed,
                session_id,
                None,
                &format!("Failed to run jcode cloud: {error}"),
            );
            return true;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let stderr = child.stderr.take();
    let progress_tx = tx.clone();
    let stderr_reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut collected = String::new();
        if let Some(stderr) = stderr {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                if let Some(rest) = line.strip_prefix("[cloud ") {
                    let detail = rest.split_once("] ").map(|(_, d)| d).unwrap_or(rest);
                    let _ = progress_tx.send(CloudEvent::Progress(detail.to_string()));
                }
                collected.push_str(&line);
                collected.push('\n');
            }
        }
        let _ = progress_tx.send(CloudEvent::Stderr(collected));
    });
    std::thread::spawn(move || {
        let output = child.wait_with_output();
        let _ = stderr_reader.join();
        let _ = tx.send(CloudEvent::Done(output.map_err(|e| e.to_string())));
    });
    let carried = carried.filter(|(index, _)| *index < app.display_messages.len());
    let (progress_message, steps) = match carried {
        Some((index, steps)) => {
            app.replace_display_message_content(index, render_cloud_steps(&steps, 0, None));
            (index, steps)
        }
        None => {
            let index = app.display_messages.len();
            let steps = vec!["Starting cloud command".to_string()];
            app.push_display_message(
                DisplayMessage::system(render_cloud_steps(&steps, 0, None))
                    .with_title("Cloud transfer"),
            );
            (index, steps)
        }
    };
    app.cloud_job = Some(CloudJob {
        progress_message,
        steps,
        started: std::time::Instant::now(),
        frame: 0,
        command: command.clone(),
        trimmed: trimmed.to_string(),
        session_id: session_id.to_string(),
        rx,
        stderr: String::new(),
        preparation_for: app
            .pending_cloud_handoff
            .clone()
            .filter(|_| matches!(command, CloudCommand::Prepare { .. })),
    });
    true
}

pub(super) enum CloudEvent {
    Progress(String),
    Stderr(String),
    Done(Result<std::process::Output, String>),
}

pub(in crate::tui) struct CloudJob {
    progress_message: usize,
    steps: Vec<String>,
    started: std::time::Instant,
    frame: usize,
    command: CloudCommand,
    trimmed: String,
    session_id: String,
    rx: std::sync::mpsc::Receiver<CloudEvent>,
    stderr: String,
    preparation_for: Option<PendingCloudHandoff>,
}

impl App {
    pub(super) fn cloud_recovery_reminder(&self) -> Option<String> {
        self.cloud_recovery_context
            .as_ref()
            .filter(|(session, _)| session == &super::commands::active_session_id(self))
            .map(|(_, context)| context.clone())
    }

    pub(super) fn take_cloud_recovery_reminder(&mut self) -> Option<String> {
        let reminder = self.cloud_recovery_reminder();
        if reminder.is_some() {
            self.cloud_recovery_context = None;
        }
        reminder
    }

    /// Drain the running job or dispatch a pending handoff when idle.
    pub(super) fn cloud_handoff_blocks_work(&self) -> bool {
        self.pending_cloud_handoff.is_some()
            || self
                .cloud_job
                .as_ref()
                .is_some_and(|job| !matches!(job.command, CloudCommand::Prepare { .. }))
            || self.cloud_handoff_requested.is_some()
    }

    fn has_unsent_handoff_work(&self, accepted_command: Option<&str>) -> bool {
        !self.unsent_handoff_work(accepted_command).is_empty()
    }

    // Report categories, never private draft contents, so an empty-looking
    // composer does not leave users guessing which queue blocked the move.
    fn unsent_handoff_work(&self, accepted_command: Option<&str>) -> Vec<&'static str> {
        let draft = self.input.trim();
        [
            (
                !draft.is_empty() && accepted_command != Some(draft),
                "composer draft",
            ),
            (!self.queued_messages.is_empty(), "queued messages"),
            (!self.queued_commands.is_empty(), "queued commands"),
            (
                !self.hidden_queued_system_messages.is_empty(),
                "queued system reminders",
            ),
            (!self.pending_images.is_empty(), "image attachments"),
            (!self.pasted_contents.is_empty(), "pasted contents"),
            (
                self.interleave_message
                    .as_ref()
                    .is_some_and(|text| !text.is_empty()),
                "pending interleave message",
            ),
            (
                !self.interleave_images.is_empty(),
                "pending interleave images",
            ),
            (
                !self.pending_soft_interrupts.is_empty(),
                "pending interrupts",
            ),
            (
                !self.pending_soft_interrupt_requests.is_empty(),
                "unacknowledged interrupts",
            ),
        ]
        .into_iter()
        .filter_map(|(present, label)| present.then_some(label))
        .collect()
    }

    /// Shared by polling and deterministic dispatch tests. No process starts here.
    fn take_ready_cloud_handoff(&mut self) -> Option<PendingCloudHandoff> {
        if !self.cloud_handoff_can_dispatch() {
            return None;
        }
        let pending = self.pending_cloud_handoff.as_ref()?;
        if matches!(pending.command, CloudCommand::Move { .. }) && !pending.prepared {
            return None;
        }
        let changed_session = pending.session_id != super::commands::active_session_id(self);
        if changed_session || self.has_unsent_handoff_work(None) {
            self.pending_cloud_handoff = None;
            let reason = if changed_session {
                "the active session changed. Retry /cloud or /local from the session you want to move".to_string()
            } else {
                format!(
                    "{} would be lost. Send or save that work, then retry /cloud or /local",
                    self.unsent_handoff_work(None).join(", ")
                )
            };
            self.push_display_message(DisplayMessage::error(format!(
                "Pending cloud handoff canceled: {reason}. Nothing was removed."
            )));
            return None;
        }
        self.pending_cloud_handoff.take()
    }

    fn cloud_handoff_can_dispatch(&self) -> bool {
        !self.is_processing
            && !self.pending_turn
            && self.cloud_job.is_none()
            && self.cloud_handoff_requested.is_none()
            && !self.should_quit
    }

    pub(super) fn poll_cloud_job(&mut self) -> bool {
        if self.cloud_job.is_none()
            && !self.should_quit
            && let Some(pending) = self.pending_cloud_handoff.as_ref()
            && !pending.prepared
            && let CloudCommand::Move { host } = &pending.command
        {
            let command = CloudCommand::Prepare { host: host.clone() };
            let trimmed = pending.trimmed.clone();
            let session_id = pending.session_id.clone();
            return start_cloud_command(self, command, &trimmed, &session_id, None);
        }
        let had_pending = self.pending_cloud_handoff.is_some();
        if let Some(pending) = self.take_ready_cloud_handoff() {
            return start_cloud_command(
                self,
                pending.command,
                &pending.trimmed,
                &pending.session_id,
                pending.progress.clone(),
            );
        }
        let Some(job) = self.cloud_job.as_mut() else {
            return had_pending && self.pending_cloud_handoff.is_none();
        };
        let mut redraw = false;
        let mut done = None;
        while let Ok(event) = job.rx.try_recv() {
            match event {
                CloudEvent::Progress(text) => {
                    push_cloud_step(&mut job.steps, text);
                    redraw = true;
                }
                CloudEvent::Stderr(text) => job.stderr = text,
                CloudEvent::Done(result) => {
                    done = Some(result);
                    break;
                }
            }
        }
        let frame = (job.started.elapsed().as_millis() / 100) as usize;
        redraw |= frame != job.frame;
        job.frame = frame;
        let finished = done
            .as_ref()
            .map(|result| result.as_ref().is_ok_and(|output| output.status.success()));
        let index = job.progress_message;
        let content = render_cloud_steps(&job.steps, frame, finished);
        if redraw || done.is_some() {
            self.replace_display_message_content(index, content);
        }
        let Some(result) = done else {
            return redraw;
        };
        // Done is sent only after the stderr reader has joined, so every
        // progress event and diagnostic has already been drained above.
        let job = self.cloud_job.take().expect("cloud job present");
        match result {
            Ok(output) => finish_cloud_command(self, job, output),
            Err(error) => {
                if job.preparation_for.as_ref() == self.pending_cloud_handoff.as_ref() {
                    self.pending_cloud_handoff = None;
                }
                show_cloud_failure(
                    self,
                    &job.trimmed,
                    &job.session_id,
                    None,
                    &format!("Failed to run jcode cloud: {error}\n{}", job.stderr),
                );
            }
        }
        true
    }
}

/// Replace the startup placeholder and skip steps already shown, so host
/// preparation and the transfer that follows read as one checklist.
#[path = "commands_cloud/diagnostics.rs"]
mod diagnostics;
use diagnostics::*;
fn finish_cloud_command(app: &mut App, job: CloudJob, output: std::process::Output) {
    if matches!(job.command, CloudCommand::Prepare { .. }) {
        // Canceled or replaced requests must never inherit stale host preparation.
        if job.preparation_for.as_ref() != app.pending_cloud_handoff.as_ref() {
            return;
        }
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if output.status.success()
            && let Some(host) = report["host"].as_str().filter(|host| !host.is_empty())
        {
            let mut steps = job.steps.clone();
            push_cloud_step(
                &mut steps,
                "cloud host ready, transfer starts at the next idle turn".to_string(),
            );
            app.replace_display_message_content(
                job.progress_message,
                render_cloud_steps(&steps, 0, Some(true)),
            );
            if let Some(pending) = app.pending_cloud_handoff.as_mut() {
                pending.command = CloudCommand::Move {
                    host: Some(host.to_string()),
                };
                pending.prepared = true;
                pending.progress = Some((job.progress_message, steps));
            }
        } else {
            app.pending_cloud_handoff = None;
            app.replace_display_message_content(
                job.progress_message,
                render_cloud_steps(&job.steps, 0, Some(false)),
            );
            let details = if output.status.success() {
                "Cloud host preparation returned an invalid response: missing non-empty host."
                    .to_string()
            } else {
                cloud_output_failure(&job, &output)
            };
            show_cloud_failure(
                app,
                &job.trimmed,
                &job.session_id,
                output.status.code(),
                &details,
            );
        }
        return;
    }
    if !output.status.success() {
        let details = cloud_output_failure(&job, &output);
        show_cloud_failure(
            app,
            &job.trimmed,
            &job.session_id,
            output.status.code(),
            &details,
        );
        return;
    }
    if matches!(
        job.command,
        CloudCommand::Move { .. } | CloudCommand::Return
    ) {
        let report = serde_json::from_slice::<serde_json::Value>(&output.stdout);
        let valid = report.as_ref().is_ok_and(|report| match job.command {
            CloudCommand::Move { .. } => {
                report["host"].as_str().is_some_and(|host| !host.is_empty())
            }
            CloudCommand::Return => report.is_object(),
            _ => unreachable!(),
        });
        if !valid {
            app.replace_display_message_content(
                job.progress_message,
                render_cloud_steps(&job.steps, 0, Some(false)),
            );
            show_cloud_failure(
                app,
                &job.trimmed,
                &job.session_id,
                output.status.code(),
                "Cloud command returned an invalid response. Could not confirm the handoff result. Run /cloud status to check the session location before retrying.",
            );
            return;
        }
    }
    let CloudJob {
        command,
        session_id,
        ..
    } = job;
    let session_id = session_id.as_str();
    if matches!(command, CloudCommand::Move { .. } | CloudCommand::Return)
        && app
            .cloud_recovery_context
            .as_ref()
            .is_some_and(|(id, _)| id == session_id)
    {
        app.cloud_recovery_context = None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    match command {
        CloudCommand::Cancel | CloudCommand::Prepare { .. } => {
            unreachable!("handled before transfer completion")
        }
        CloudCommand::Where => {
            let text = if stdout.trim().is_empty() {
                "This session has never moved.".to_string()
            } else {
                stdout.trim().to_string()
            };
            app.push_display_message(DisplayMessage::system(text));
        }
        CloudCommand::Move { .. } => {
            let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
            let host = report["host"].as_str().unwrap_or_default().to_string();
            let repo_root = report["repo_root"].as_str().map(str::to_string);
            app.push_display_message(DisplayMessage::system(format!(
                "☁️ Session moved to `{host}`. Reattaching there and continuing. Its disk persists while it sleeps. Use /local to bring it back. Your local files stay editable, and git merges both sides on return."
            )));
            app.cloud_handoff_requested = Some(CloudHandoff::Remote {
                session_id: session_id.to_string(),
                host,
                working_dir: repo_root,
            });
            app.should_quit = true;
        }
        CloudCommand::Return => {
            let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
            let merge = &report["merge"];
            let note = match merge["result"].as_str() {
                Some("applied") => "Cloud work was merged into your checkout.".to_string(),
                Some("unchanged") => "No code changed on the cloud side.".to_string(),
                Some("conflicts") => format!(
                    "Code conflicts, working tree untouched: {}. The agent can resolve them.",
                    merge["files"]
                        .as_array()
                        .map(|f| f
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                ),
                _ => String::new(),
            };
            app.push_display_message(DisplayMessage::system(format!(
                "🏠 Session is back on this machine. {note}"
            )));
            app.cloud_handoff_requested = Some(CloudHandoff::Local {
                session_id: session_id.to_string(),
            });
            app.should_quit = true;
        }
    }
}

#[cfg(test)]
#[path = "commands_cloud/tests.rs"]
mod tests;
