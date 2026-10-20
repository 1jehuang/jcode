//! `/cloud` and `/local`: move the current session to a cloud host and back.
//!
//! The heavy lifting is the `jcode cloud move|return` CLI. The TUI only runs it
//! for the current session between turns, surfaces the result in the chat, and
//! then reattaches at the new location (SSH attach for `/cloud`, local resume
//! for `/local`). A failure leaves everything where it was.

use super::{App, CloudHandoff, DisplayMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CloudCommand {
    Move { host: Option<String> },
    Return,
    Where,
}

pub(super) fn parse_cloud_command(trimmed: &str) -> Option<CloudCommand> {
    let mut words = trimmed.split_whitespace();
    match words.next()? {
        "/cloud" => match words.next() {
            None => Some(CloudCommand::Move { host: None }),
            Some("status") | Some("where") => Some(CloudCommand::Where),
            Some("back") | Some("return") => Some(CloudCommand::Return),
            Some(host) => Some(CloudCommand::Move {
                host: Some(host.to_string()),
            }),
        },
        "/local" => Some(CloudCommand::Return),
        _ => None,
    }
}

/// Run a `/cloud` family command. Returns true when handled.
pub(super) fn handle_cloud_command(app: &mut App, trimmed: &str, session_id: &str) -> bool {
    let Some(command) = parse_cloud_command(trimmed) else {
        return false;
    };
    if app.is_processing && !matches!(command, CloudCommand::Where) {
        app.push_display_message(DisplayMessage::error(
            "The agent is mid-turn. Wait for it to finish (or press Esc), then run the command again. The move happens between turns so nothing is lost.".to_string(),
        ));
        return true;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
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
        command,
        trimmed: trimmed.to_string(),
        session_id: session_id.to_string(),
        rx,
        stderr: String::new(),
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
}

impl App {
    /// Drain the running `/cloud` job. Returns true when the UI should redraw.
    pub(super) fn poll_cloud_job(&mut self) -> bool {
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
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Failed to run jcode cloud: {error}"
            ))),
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
        assert_eq!(parse_cloud_command("/cloudy"), None);
    }
}
