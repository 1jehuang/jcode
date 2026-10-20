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
            app.set_status_notice("Moving session to the cloud...");
        }
        CloudCommand::Return => {
            cmd.args(["return", "--json", "--session", session_id]);
            app.set_status_notice("Bringing session back...");
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
    std::thread::spawn(move || {
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
        let _ = tx.send(CloudEvent::Done(output.map_err(|e| e.to_string())));
    });
    app.cloud_job = Some(CloudJob {
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
        let mut progress = None;
        while let Ok(event) = job.rx.try_recv() {
            match event {
                CloudEvent::Progress(text) => progress = Some(text),
                CloudEvent::Stderr(text) => job.stderr = text,
                CloudEvent::Done(result) => {
                    done = Some(result);
                    break;
                }
            }
        }
        if let Some(text) = progress {
            self.set_status_notice(format!("☁ {text}"));
            redraw = true;
        }
        let Some(result) = done else {
            return redraw;
        };
        // Collect any trailing stderr the reader thread produced.
        let mut job = self.cloud_job.take().expect("cloud job present");
        if let Ok(CloudEvent::Stderr(text)) =
            job.rx.recv_timeout(std::time::Duration::from_millis(500))
        {
            job.stderr = text;
        }
        match result {
            Ok(output) => finish_cloud_command(self, job, output),
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Failed to run jcode cloud: {error}"
            ))),
        }
        true
    }
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
        let stderr = stderr
            .lines()
            .filter(|l| !l.starts_with("[cloud "))
            .collect::<Vec<_>>()
            .join("\n");
        app.push_display_message(DisplayMessage::error(format!(
            "{} failed. Nothing moved, the session is still here.\n{}",
            trimmed,
            stderr.trim()
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
