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
        app.pending_cloud_handoff = Some(PendingCloudHandoff {
            command,
            trimmed: trimmed.to_string(),
            session_id: session_id.to_string(),
            request_id: next_request_id(),
            prepared: false,
        });
        app.push_display_message(DisplayMessage::system(
            "Cloud handoff queued for the next idle turn boundary. The current turn continues here. Use /cloud cancel to cancel.",
        ));
        app.poll_cloud_job();
        return true;
    }
    start_cloud_command(app, command, trimmed, session_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct PendingCloudHandoff {
    command: CloudCommand,
    trimmed: String,
    session_id: String,
    request_id: u64,
    prepared: bool,
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
) -> bool {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            if matches!(command, CloudCommand::Prepare { .. }) {
                app.pending_cloud_handoff = None;
            }
            app.push_display_message(DisplayMessage::error(format!(
                "Cannot locate the jcode binary: {error}"
            )));
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
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to run jcode cloud: {error}"
            )));
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
    let progress_message = app.display_messages.len();
    let steps = vec!["Starting cloud command".to_string()];
    app.push_display_message(
        DisplayMessage::system(render_cloud_steps(&steps, 0, None)).with_title("Cloud transfer"),
    );
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
    /// Drain the running job or dispatch a pending handoff when idle.
    pub(super) fn cloud_handoff_blocks_work(&self) -> bool {
        self.pending_cloud_handoff.is_some()
            || self.cloud_job.is_some()
            || self.cloud_handoff_requested.is_some()
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
            return start_cloud_command(
                self,
                CloudCommand::Prepare { host: host.clone() },
                "/cloud prepare",
                "",
            );
        }
        if self.cloud_handoff_can_dispatch()
            && let Some(pending) = self.pending_cloud_handoff.take()
        {
            return start_cloud_command(
                self,
                pending.command,
                &pending.trimmed,
                &pending.session_id,
            );
        }
        let Some(job) = self.cloud_job.as_mut() else {
            return false;
        };
        let mut redraw = false;
        let mut done = None;
        while let Ok(event) = job.rx.try_recv() {
            match event {
                CloudEvent::Progress(text) => {
                    job.steps.push(text);
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
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to run jcode cloud: {error}"
                )));
            }
        }
        true
    }
}

fn render_cloud_steps(steps: &[String], frame: usize, finished: Option<bool>) -> String {
    const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut lines = vec!["Cloud session transfer".to_string()];
    for (index, step) in steps.iter().enumerate() {
        let last = index + 1 == steps.len();
        let marker = if !last || finished == Some(true) {
            "✓"
        } else if finished == Some(false) {
            "✗"
        } else {
            SPINNER[frame % SPINNER.len()]
        };
        lines.push(format!("{marker} {}. {step}", index + 1));
    }
    lines.join("\n")
}

/// Keep remote diagnostics literal so Markdown cannot fold their newlines.
fn format_cloud_failure(command: &str, stderr: &str) -> String {
    let details = stderr
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.starts_with("[cloud ")
                && !line.starts_with(
                    "** WARNING: connection is not using a post-quantum key exchange algorithm.",
                )
                && !line.starts_with(
                    "** This session may be vulnerable to \"store now, decrypt later\" attacks.",
                )
                && !(line.starts_with("** The server may need to be upgraded.")
                    && line.contains("openssh.com/pq.html"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let details = details.trim();
    let mut message = format!("{command} failed. Nothing moved, the session is still here.");
    if details.is_empty() {
        message.push_str("\n\nNo error details were returned by the cloud command.");
        return message;
    }
    // Nested fences in diagnostics must not break out of the literal block.
    let fence = "`".repeat(
        details
            .split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    message.push_str(&format!("\n\n{fence}text\n{details}\n{fence}"));
    if (details.contains("Saved account '") && details.contains("' no longer exists."))
        || ((details.contains("No account with label '")
            || details.contains("No OpenAI account with label '"))
            && details.contains("' found"))
    {
        message.push_str("\n\nThe session's saved provider account is missing on the cloud host. Sign in to that provider on the cloud host using the same account label, or use /model locally to select an account already configured there, then retry. Provider credentials are not copied by cloud moves.");
    }
    message
}

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
            if let Some(pending) = app.pending_cloud_handoff.as_mut() {
                pending.command = CloudCommand::Move {
                    host: Some(host.to_string()),
                };
                pending.prepared = true;
            }
            app.push_display_message(DisplayMessage::system(
                "Cloud host ready. Transfer will start at the next idle turn boundary.",
            ));
        } else {
            app.pending_cloud_handoff = None;
            app.push_display_message(DisplayMessage::error(format!(
                "Cloud host preparation failed. Pending handoff cleared; retry /cloud when ready.\n{}",
                job.stderr.trim()
            )));
        }
        return;
    }
    let CloudJob {
        command,
        trimmed,
        session_id,
        stderr,
        ..
    } = job;
    let session_id = session_id.as_str();
    let trimmed = trimmed.as_str();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        app.push_display_message(DisplayMessage::error(format_cloud_failure(
            trimmed, &stderr,
        )));
        app.set_status_notice("Cloud move failed");
        return;
    }
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
mod tests {
    use super::*;

    fn pending(command: CloudCommand, session_id: &str) -> PendingCloudHandoff {
        PendingCloudHandoff {
            command,
            trimmed: "/cloud chosen".into(),
            session_id: session_id.into(),
            request_id: next_request_id(),
            prepared: false,
        }
    }

    fn fake_job(app: &mut App, command: CloudCommand) -> std::sync::mpsc::Sender<CloudEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        let progress_message = app.display_messages.len();
        app.push_display_message(DisplayMessage::system("Cloud session transfer"));
        app.cloud_job = Some(CloudJob {
            progress_message,
            steps: vec!["Starting cloud command".into()],
            started: std::time::Instant::now(),
            frame: 0,
            preparation_for: app
                .pending_cloud_handoff
                .clone()
                .filter(|_| matches!(command, CloudCommand::Prepare { .. })),
            command,
            trimmed: "/cloud".into(),
            session_id: "original-session".into(),
            rx,
            stderr: String::new(),
        });
        tx
    }

    #[cfg(unix)]
    fn output(success: bool, stdout: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn busy_return_queues_without_spawning_and_status_preserves_target() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_processing = true;
        assert!(handle_cloud_command(&mut app, "/local", "original-session"));
        let expected = app.pending_cloud_handoff.clone();
        assert!(
            app.cloud_job.is_none(),
            "no transfer child may start during a turn"
        );
        assert!(!app.poll_cloud_job());
        assert!(handle_cloud_command(
            &mut app,
            "/cloud status",
            "different-session"
        ));
        assert_eq!(app.pending_cloud_handoff, expected);
        assert_eq!(expected.unwrap().session_id, "original-session");
        assert!(app.cloud_handoff_blocks_work());
        app.input = "do more work".into();
        app.submit_input();
        assert_eq!(app.input, "do more work");
        assert!(!app.pending_turn);
        app.queued_commands.push("/cloud cancel".into());
        app.queued_messages.push("queued followup".into());
        app.is_processing = false;
        super::super::input::run_queued_commands_local(&mut app);
        assert_eq!(app.queued_commands, ["/cloud cancel"]);
        assert_eq!(app.queued_messages, ["queued followup"]);
        assert!(app.pending_cloud_handoff.is_some());
        app.is_processing = true;
        for cancel in ["/cloud cancel", "/local cancel"] {
            handle_cloud_command(&mut app, cancel, "different-session");
            assert!(app.pending_cloud_handoff.is_none());
            assert!(app.cloud_job.is_none());
            handle_cloud_command(&mut app, "/local", "original-session");
        }
    }

    #[test]
    fn dispatch_waits_for_idle_and_staged_turn_and_completed_handoff() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.pending_cloud_handoff = Some(pending(CloudCommand::Return, "original-session"));
        app.is_processing = true;
        assert!(!app.cloud_handoff_can_dispatch());
        app.is_processing = false;
        app.pending_turn = true;
        assert!(!app.cloud_handoff_can_dispatch());
        assert!(!app.poll_cloud_job());
        assert!(app.cloud_job.is_none());
        app.pending_turn = false;
        assert!(app.cloud_handoff_can_dispatch());
        let _tx = fake_job(&mut app, CloudCommand::Return);
        assert!(!app.cloud_handoff_can_dispatch());
        app.cloud_job = None;
        app.cloud_handoff_requested = Some(CloudHandoff::Local {
            session_id: "original-session".into(),
        });
        assert!(!app.cloud_handoff_can_dispatch());
        app.cloud_handoff_requested = None;
        app.should_quit = true;
        assert!(!app.cloud_handoff_can_dispatch());
    }

    #[cfg(unix)]
    #[test]
    fn prepared_host_is_pinned_without_starting_busy_transfer() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_processing = true;
        app.pending_cloud_handoff = Some(pending(
            CloudCommand::Move { host: None },
            "original-session",
        ));
        let tx = fake_job(&mut app, CloudCommand::Prepare { host: None });
        tx.send(CloudEvent::Done(Ok(output(
            true,
            r#"{"host":"resolved-host"}"#,
        ))))
        .unwrap();
        assert!(app.poll_cloud_job());
        let queued = app.pending_cloud_handoff.as_ref().unwrap();
        assert_eq!(
            queued.command,
            CloudCommand::Move {
                host: Some("resolved-host".into())
            }
        );
        assert_eq!(queued.session_id, "original-session");
        assert!(queued.prepared);
        assert!(!app.poll_cloud_job());
        assert!(app.cloud_job.is_none());
        assert!(app.cloud_handoff_requested.is_none());
        assert!(!app.should_quit);
        app.is_processing = false;
        assert!(app.cloud_handoff_can_dispatch());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_and_cancel_ignore_stale_preparation_results() {
        for cancel in [false, true] {
            let mut app = crate::tui::app::tests::create_test_app();
            app.is_processing = true;
            app.pending_cloud_handoff = Some(pending(
                CloudCommand::Move {
                    host: Some("old".into()),
                },
                "original-session",
            ));
            let tx = fake_job(
                &mut app,
                CloudCommand::Prepare {
                    host: Some("old".into()),
                },
            );
            handle_cloud_command(&mut app, "/cloud new", "replacement-session");
            let expected = app.pending_cloud_handoff.clone();
            assert_eq!(
                expected.as_ref().unwrap().command,
                CloudCommand::Move {
                    host: Some("new".into())
                }
            );
            if cancel {
                handle_cloud_command(&mut app, "/local cancel", "replacement-session");
            }
            tx.send(CloudEvent::Done(Ok(output(true, r#"{"host":"old"}"#))))
                .unwrap();
            assert!(app.poll_cloud_job());
            assert_eq!(
                app.pending_cloud_handoff,
                if cancel { None } else { expected }
            );
            assert!(app.cloud_handoff_requested.is_none());
            assert!(!app.should_quit);
        }
    }

    #[cfg(unix)]
    #[test]
    fn preparation_failure_clears_only_its_own_request() {
        for replace in [false, true] {
            let mut app = crate::tui::app::tests::create_test_app();
            app.is_processing = true;
            app.pending_cloud_handoff = Some(pending(
                CloudCommand::Move { host: None },
                "original-session",
            ));
            let tx = fake_job(&mut app, CloudCommand::Prepare { host: None });
            if replace {
                handle_cloud_command(&mut app, "/local", "replacement-session");
            }
            tx.send(CloudEvent::Done(Ok(output(false, "")))).unwrap();
            app.poll_cloud_job();
            assert_eq!(app.pending_cloud_handoff.is_some(), replace);
            assert!(app.cloud_job.is_none());
        }
    }

    #[test]
    fn active_transfer_rejects_replacement_and_cancellation_without_killing_job() {
        let mut app = crate::tui::app::tests::create_test_app();
        let _tx = fake_job(&mut app, CloudCommand::Return);
        handle_cloud_command(&mut app, "/cloud another", "different-session");
        handle_cloud_command(&mut app, "/cloud cancel", "different-session");
        assert!(app.pending_cloud_handoff.is_none());
        assert_eq!(
            app.cloud_job.as_ref().unwrap().command,
            CloudCommand::Return
        );
        assert!(app.cloud_handoff_blocks_work());
    }

    #[cfg(unix)]
    #[test]
    fn polling_keeps_all_steps_and_stops_spinner_on_failure() {
        use std::os::unix::process::ExitStatusExt;
        let mut app = crate::tui::app::tests::create_test_app();
        let index = app.display_messages.len();
        app.push_display_message(DisplayMessage::system("Cloud session transfer"));
        let (tx, rx) = std::sync::mpsc::channel();
        app.cloud_job = Some(CloudJob {
            progress_message: index,
            steps: vec!["Starting cloud command".into()],
            started: std::time::Instant::now(),
            frame: 0,
            command: CloudCommand::Move { host: None },
            trimmed: "/cloud".into(),
            session_id: "test".into(),
            rx,
            stderr: String::new(),
            preparation_for: None,
        });
        tx.send(CloudEvent::Progress("Checking host".into()))
            .unwrap();
        tx.send(CloudEvent::Progress("Uploading session".into()))
            .unwrap();
        assert!(app.poll_cloud_job());
        let content = &app.display_messages[index].content;
        assert!(content.contains("✓ 2. Checking host"));
        assert!(content.contains("3. Uploading session"));
        let before = content.clone();
        app.cloud_job.as_mut().unwrap().started =
            std::time::Instant::now() - std::time::Duration::from_millis(250);
        assert!(app.poll_cloud_job());
        let after = &app.display_messages[index].content;
        assert_ne!(
            &before, after,
            "active spinner must advance without new events"
        );
        assert!(after.contains("✓ 2. Checking host"));
        tx.send(CloudEvent::Stderr("Error: test failure".into()))
            .unwrap();
        tx.send(CloudEvent::Done(Ok(std::process::Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })))
        .unwrap();
        assert!(app.poll_cloud_job());
        assert!(app.cloud_job.is_none());
        assert!(
            app.display_messages[index]
                .content
                .contains("✗ 3. Uploading session")
        );
        assert!(!app.poll_cloud_job());
    }

    #[test]
    fn cloud_markdown_keeps_steps_and_diagnostics_on_separate_rows() {
        let steps = vec!["Preparing host".into(), "Uploading session".into()];
        let content = render_cloud_steps(&steps, 0, None);
        let ordinary = crate::tui::ui::render_system_message(
            &DisplayMessage::system(content.clone()),
            80,
            crate::config::DiffDisplayMode::Off,
        );
        assert_eq!(
            ordinary.len(),
            1,
            "ordinary notices are intentionally compact"
        );
        let message = DisplayMessage::system(content).with_title("Cloud transfer");
        let rendered = crate::tui::ui::render_system_message(
            &message,
            80,
            crate::config::DiffDisplayMode::Off,
        );
        let rows: Vec<String> = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        let completed = rows
            .iter()
            .position(|row| row.contains("✓ 1. Preparing host"))
            .unwrap();
        let active = rows
            .iter()
            .position(|row| row.contains("⠋ 2. Uploading session"))
            .unwrap();
        assert!(
            completed < active,
            "each step must occupy its own rendered row: {rows:?}"
        );
        assert!(!rows[completed].contains("Uploading"));
        println!(
            "Cloud layout comparison: ordinary notice={} row; cloud report={} rows\n{}",
            ordinary.len(),
            rows.len(),
            rows.join("\n"),
        );

        let error = format_cloud_failure(
            "/cloud",
            "Error: remote failed\n\nCaused by:\n    No OpenAI account with label 'work' found",
        );
        let rendered = crate::tui::markdown::render_markdown_with_width(&error, Some(80));
        let rows: Vec<String> = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        let error_row = rows
            .iter()
            .position(|row| row.contains("Error: remote failed"))
            .unwrap();
        let cause_row = rows
            .iter()
            .position(|row| row.contains("Caused by:"))
            .unwrap();
        let account_row = rows
            .iter()
            .position(|row| row.contains("No OpenAI account"))
            .unwrap();
        assert!(
            error_row < cause_row && cause_row < account_row,
            "diagnostic lines must not collapse: {rows:?}"
        );
    }

    #[test]
    fn cloud_transcript_preserves_steps_and_animates_only_active_step() {
        let steps = vec!["Preparing host".into(), "Uploading session".into()];
        let first = render_cloud_steps(&steps, 0, None);
        let next = render_cloud_steps(&steps, 1, None);
        assert!(first.contains("✓ 1. Preparing host"));
        assert!(first.contains("⠋ 2. Uploading session"));
        assert!(next.contains("⠙ 2. Uploading session"));
        assert!(render_cloud_steps(&steps, 0, Some(true)).contains("✓ 2. Uploading session"));
        let failed = render_cloud_steps(&steps, 0, Some(false));
        assert!(failed.contains("✓ 1. Preparing host"));
        assert!(failed.contains("✗ 2. Uploading session"));
    }

    #[test]
    fn cloud_failure_strips_only_known_boilerplate() {
        let message = format_cloud_failure(
            "/cloud",
            concat!(
                "[cloud prepare] checking host\n",
                "** WARNING: connection is not using a post-quantum key exchange algorithm.\n",
                "** This session may be vulnerable to \"store now, decrypt later\" attacks.\n",
                "** The server may need to be upgraded. See https://openssh.com/pq.html\n",
                "Error: remote failed\n\nCaused by:\n    Permission denied (publickey).\n",
                "WARNING: host key changed\n",
            ),
        );
        assert!(!message.contains("[cloud"));
        assert!(!message.contains("post-quantum"));
        assert!(!message.contains("store now"));
        assert!(!message.contains("openssh.com"));
        assert!(message.contains("```text\nError: remote failed\n\nCaused by:\n    Permission denied (publickey).\nWARNING: host key changed\n```"));
        assert!(!message.contains("Sign in"));
    }

    #[test]
    fn cloud_failure_explains_missing_saved_account() {
        for error in [
            "Saved account 'work' no longer exists. Select an account with /model.",
            "No account with label 'work' found",
            "No OpenAI account with label 'work' found",
        ] {
            let message = format_cloud_failure("/cloud", error);
            assert!(message.contains(error));
            assert!(message.contains("missing on the cloud host"));
            assert!(message.contains("use /model locally"));
            assert!(message.contains("credentials are not copied"));
        }
    }

    #[test]
    fn cloud_failure_handles_empty_details_and_nested_fences() {
        let message = format_cloud_failure("/local", "[cloud export] exporting\n");
        assert!(message.starts_with("/local failed."));
        assert!(message.contains("No error details were returned"));
        assert!(!message.contains("```"));
        let message = format_cloud_failure("/cloud", "Error:\r\n```shell\r\nfailed\r\n```\r\n");
        assert!(message.contains("````text\nError:\n```shell\nfailed\n```\n````"));
    }

    #[test]
    fn parses_cloud_commands() {
        assert_eq!(
            parse_cloud_command("/cloud"),
            Some(CloudCommand::Move { host: None })
        );
        assert_eq!(
            parse_cloud_command("/cloud my-box"),
            Some(CloudCommand::Move {
                host: Some("my-box".into())
            })
        );
        assert_eq!(parse_cloud_command("/local"), Some(CloudCommand::Return));
        assert_eq!(
            parse_cloud_command("/cloud back"),
            Some(CloudCommand::Return)
        );
        assert_eq!(
            parse_cloud_command("/cloud status"),
            Some(CloudCommand::Where)
        );
        assert_eq!(
            parse_cloud_command("/cloud cancel"),
            Some(CloudCommand::Cancel)
        );
        assert_eq!(
            parse_cloud_command("/local cancel"),
            Some(CloudCommand::Cancel)
        );
        assert_eq!(parse_cloud_command("/cloudy"), None);
    }
}
