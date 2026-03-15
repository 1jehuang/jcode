use super::*;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

impl App {
    pub(crate) fn handle_dictation_trigger(&mut self) -> bool {
        let cfg = crate::config::config().dictation.clone();
        let command = cfg.command.trim().to_string();

        if command.is_empty() {
            self.push_display_message(DisplayMessage::error(
                "Dictation is not configured. Set `[dictation].command` in `~/.jcode/config.toml`."
                    .to_string(),
            ));
            self.set_status_notice("Dictation not configured");
            return true;
        }

        if self.dictation_in_flight {
            self.set_status_notice("Dictation already running");
            return true;
        }

        self.dictation_in_flight = true;
        self.set_status_notice("🎙 Starting dictation...");

        tokio::spawn(async move {
            match run_dictation_command(&command, cfg.timeout_secs).await {
                Ok(text) => Bus::global().publish(BusEvent::DictationCompleted {
                    text,
                    mode: cfg.mode,
                }),
                Err(error) => Bus::global().publish(BusEvent::DictationFailed { message: error }),
            }
        });

        true
    }

    pub(crate) fn dictation_key_matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.dictation_key
            .binding
            .as_ref()
            .map(|binding| binding.matches(code, modifiers))
            .unwrap_or(false)
    }

    pub(crate) fn dictation_key_label(&self) -> Option<&str> {
        self.dictation_key.label.as_deref()
    }

    pub(crate) fn handle_dictation_failure(&mut self, message: String) {
        self.dictation_in_flight = false;
        self.push_display_message(DisplayMessage::error(format!(
            "Dictation failed: {}",
            message
        )));
        self.set_status_notice("Dictation failed");
    }

    pub(crate) fn handle_local_dictation_completed(
        &mut self,
        text: String,
        mode: crate::protocol::TranscriptMode,
    ) {
        self.dictation_in_flight = false;
        super::remote::apply_transcript_event(self, text, mode);
    }

    pub(crate) fn mark_dictation_delivered(&mut self) {
        self.dictation_in_flight = false;
    }
}

async fn run_dictation_command(command: &str, timeout_secs: u64) -> Result<String, String> {
    let mut child = shell_command(command);
    child.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = child
        .spawn()
        .map_err(|e| format!("failed to start `{}`: {}", command, e))?;

    let output = if timeout_secs == 0 {
        child
            .wait_with_output()
            .await
            .map_err(|e| format!("failed to wait for dictation command: {}", e))?
    } else {
        timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
            .map_err(|_| format!("timed out after {}s", timeout_secs))?
            .map_err(|e| format!("failed to wait for dictation command: {}", e))?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stderr
        };
        return Err(detail);
    }

    let transcript = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .trim()
        .to_string();

    if transcript.is_empty() {
        return Err("command returned an empty transcript".to_string());
    }

    Ok(transcript)
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-lc").arg(command);
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::run_dictation_command;

    #[tokio::test]
    async fn dictation_command_trims_trailing_newlines() {
        let text = run_dictation_command("printf 'hello from test\\n'", 5)
            .await
            .expect("dictation command should succeed");
        assert_eq!(text, "hello from test");
    }
}
