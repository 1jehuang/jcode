use super::*;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant, sleep};

pub(crate) struct ActiveDictation {
    pid: u32,
    #[cfg(not(unix))]
    child: Arc<Mutex<Option<Child>>>,
}

impl ActiveDictation {
    fn new(pid: u32, _child: Arc<Mutex<Option<Child>>>) -> Self {
        Self {
            pid,
            #[cfg(not(unix))]
            child: _child,
        }
    }

    async fn request_stop(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            crate::platform::signal_detached_process_group(self.pid, libc::SIGINT)
                .map_err(|e| format!("failed to stop dictation: {}", e))
        }
        #[cfg(not(unix))]
        {
            let mut guard = self.child.lock().await;
            let Some(child) = guard.as_mut() else {
                return Ok(());
            };
            child
                .start_kill()
                .map_err(|e| format!("failed to stop dictation: {}", e))
        }
    }
}

#[derive(Debug)]
enum DictationExit {
    Exited(ExitStatus),
    TimedOut,
    WaitError(String),
}

impl App {
    pub(crate) fn handle_dictation_trigger(&mut self) -> bool {
        let cfg = crate::config::config().dictation.clone();
        let command = cfg.command.trim().to_string();
        let target_session_id = self.active_client_session_id().map(str::to_string);

        if command.is_empty() {
            self.push_display_message(DisplayMessage::error(
                "Dictation is not configured. Set `[dictation].command` in `~/.jcode/config.toml`."
                    .to_string(),
            ));
            self.set_status_notice("Dictation not configured");
            return true;
        }

        if self.dictation_in_flight {
            if let Some(active) = self.dictation_session.take() {
                let dictation_id = self.dictation_request_id.clone().unwrap_or_default();
                let session_id = self.dictation_target_session_id.clone();
                self.set_status_notice("🎙 Stopping dictation...");
                tokio::spawn(async move {
                    if let Err(error) = active.request_stop().await {
                        Bus::global().publish(BusEvent::DictationFailed {
                            dictation_id,
                            session_id,
                            message: error,
                        });
                    }
                });
            } else {
                self.set_status_notice("Dictation already running");
            }
            return true;
        }

        self.note_client_focus(true);

        let mut child = shell_command(&command);
        child.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match child.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Dictation failed: failed to start `{}`: {}",
                    command, error
                )));
                self.set_status_notice("Dictation failed");
                return true;
            }
        };

        let pid = match child.id() {
            Some(pid) => pid,
            None => {
                self.push_display_message(DisplayMessage::error(
                    "Dictation failed: spawned process has no PID".to_string(),
                ));
                self.set_status_notice("Dictation failed");
                return true;
            }
        };

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                self.push_display_message(DisplayMessage::error(
                    "Dictation failed: could not capture stdout".to_string(),
                ));
                self.set_status_notice("Dictation failed");
                return true;
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                self.push_display_message(DisplayMessage::error(
                    "Dictation failed: could not capture stderr".to_string(),
                ));
                self.set_status_notice("Dictation failed");
                return true;
            }
        };

        let child = Arc::new(Mutex::new(Some(child)));
        let dictation_id = crate::id::new_id("dictation");
        self.dictation_session = Some(ActiveDictation::new(pid, Arc::clone(&child)));
        self.dictation_in_flight = true;
        self.dictation_request_id = Some(dictation_id.clone());
        self.dictation_target_session_id = target_session_id.clone();
        self.set_status_notice("🎙 Dictation running — press again to stop");

        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));
        let stdout_task = tokio::spawn(read_stream_into_buffer(stdout, Arc::clone(&stdout_buf)));
        let stderr_task = tokio::spawn(read_stream_into_buffer(stderr, Arc::clone(&stderr_buf)));

        tokio::spawn(async move {
            let exit = wait_for_dictation_exit(Arc::clone(&child), cfg.timeout_secs).await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            publish_dictation_result(
                dictation_id,
                target_session_id,
                command,
                cfg.mode,
                exit,
                stdout_buf,
                stderr_buf,
            )
            .await;
        });

        true
    }

    pub(crate) fn handle_empty_clipboard_paste(&mut self) -> bool {
        let cfg = crate::config::config().dictation.clone();
        if should_fallback_from_empty_clipboard(
            cfg.command.as_str(),
            self.dictation_key.binding.is_some(),
        ) {
            return self.handle_dictation_trigger();
        }
        false
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
        self.clear_dictation_tracking();
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
        self.clear_dictation_tracking();
        super::remote::apply_transcript_event(self, text, mode);
    }

    pub(crate) fn mark_dictation_delivered(&mut self) {
        self.clear_dictation_tracking();
    }

    pub(crate) fn owns_dictation_event(
        &self,
        dictation_id: &str,
        session_id: Option<&str>,
    ) -> bool {
        self.dictation_in_flight
            && self.dictation_request_id.as_deref() == Some(dictation_id)
            && self.dictation_target_session_id.as_deref() == session_id
    }

    fn clear_dictation_tracking(&mut self) {
        self.dictation_in_flight = false;
        self.dictation_session = None;
        self.dictation_request_id = None;
        self.dictation_target_session_id = None;
    }
}

async fn read_stream_into_buffer<R>(mut reader: R, buffer: Arc<Mutex<Vec<u8>>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buffer.lock().await.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
}

async fn wait_for_dictation_exit(
    child: Arc<Mutex<Option<Child>>>,
    timeout_secs: u64,
) -> DictationExit {
    let started = Instant::now();
    loop {
        let poll = {
            let mut guard = child.lock().await;
            let Some(child) = guard.as_mut() else {
                return DictationExit::WaitError("dictation process disappeared".to_string());
            };
            child.try_wait()
        };

        match poll {
            Ok(Some(status)) => return DictationExit::Exited(status),
            Ok(None) => {}
            Err(error) => return DictationExit::WaitError(error.to_string()),
        }

        if timeout_secs > 0 && started.elapsed() >= Duration::from_secs(timeout_secs) {
            let pid = {
                let guard = child.lock().await;
                guard.as_ref().and_then(|process| process.id())
            };
            if let Some(pid) = pid {
                #[cfg(unix)]
                {
                    let _ = crate::platform::signal_detached_process_group(pid, libc::SIGINT);
                }
                #[cfg(not(unix))]
                {
                    let mut guard = child.lock().await;
                    if let Some(process) = guard.as_mut() {
                        let _ = process.start_kill();
                    }
                }
            }

            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let poll = {
                    let mut guard = child.lock().await;
                    let Some(child) = guard.as_mut() else {
                        return DictationExit::TimedOut;
                    };
                    child.try_wait()
                };
                match poll {
                    Ok(Some(_)) => return DictationExit::TimedOut,
                    Ok(None) => {}
                    Err(error) => return DictationExit::WaitError(error.to_string()),
                }
                if Instant::now() >= deadline {
                    let mut guard = child.lock().await;
                    if let Some(process) = guard.as_mut() {
                        let _ = process.start_kill();
                    }
                    return DictationExit::TimedOut;
                }
                sleep(Duration::from_millis(50)).await;
            }
        }

        sleep(Duration::from_millis(50)).await;
    }
}

async fn publish_dictation_result(
    dictation_id: String,
    session_id: Option<String>,
    command: String,
    mode: crate::protocol::TranscriptMode,
    exit: DictationExit,
    stdout_buf: Arc<Mutex<Vec<u8>>>,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
) {
    let stdout = String::from_utf8_lossy(&stdout_buf.lock().await).to_string();
    let stderr = String::from_utf8_lossy(&stderr_buf.lock().await).to_string();

    match transcript_from_command_output(&stdout) {
        Some(text) => {
            Bus::global().publish(BusEvent::DictationCompleted {
                dictation_id,
                session_id,
                text,
                mode,
            });
        }
        None => {
            let message = match exit {
                DictationExit::Exited(status) if !status.success() => {
                    let stderr = stderr.trim();
                    if stderr.is_empty() {
                        format!("dictation command `{}` exited with {}", command, status)
                    } else {
                        stderr.to_string()
                    }
                }
                DictationExit::TimedOut => format!(
                    "dictation command `{}` timed out before producing a transcript",
                    command
                ),
                DictationExit::WaitError(error) => {
                    format!("failed while waiting for dictation command: {}", error)
                }
                DictationExit::Exited(_) => {
                    let stderr = stderr.trim();
                    if stderr.is_empty() {
                        "dictation command returned an empty transcript".to_string()
                    } else {
                        stderr.to_string()
                    }
                }
            };
            Bus::global().publish(BusEvent::DictationFailed {
                dictation_id,
                session_id,
                message,
            });
        }
    }
}

#[cfg(test)]
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
        tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
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

    transcript_from_command_output(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "command returned an empty transcript".to_string())
}

fn transcript_from_command_output(stdout: &str) -> Option<String> {
    let cleaned = strip_ansi(stdout).replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() || is_status_only_line(line) {
            continue;
        }

        if let Some(translation) = line.strip_prefix('→').map(str::trim) {
            if !translation.is_empty() {
                if !lines.is_empty() {
                    lines.pop();
                }
                lines.push(translation.to_string());
            }
            continue;
        }

        if line.starts_with('拼') {
            continue;
        }

        let content = strip_transcript_prefixes(line);
        if !content.is_empty() {
            lines.push(content.to_string());
        }
    }

    let transcript = lines.join(" ").trim().to_string();
    (!transcript.is_empty()).then_some(transcript)
}

fn strip_transcript_prefixes(line: &str) -> &str {
    let Some(rest) = line.strip_prefix('[') else {
        return line;
    };
    let Some((_, rest)) = rest.split_once(']') else {
        return line;
    };
    let rest = rest.trim_start();
    if let Some(rest) = rest.strip_prefix('[')
        && let Some((_, rest)) = rest.split_once(']')
    {
        return rest.trim_start();
    }
    line
}

fn is_status_only_line(line: &str) -> bool {
    line == "=================================================="
        || line.starts_with("Loading WebRTC VAD")
        || line.contains("Live transcription started")
        || line.starts_with('🎤')
        || line.starts_with('📝')
        || line.starts_with("Saving to:")
        || line.starts_with('🌐')
        || line.starts_with("Auto-translating")
        || line.starts_with('🀄')
        || line.starts_with("Pinyin shown")
        || line.starts_with('🎯')
        || line.starts_with("Silence threshold:")
        || line.starts_with("Listening...")
        || line.contains("Recording...")
        || line == "Transcription stopped."
}

fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if next == '\u{7}' || (prev == '\u{1b}' && next == '\\') {
                            break;
                        }
                        prev = next;
                    }
                }
                _ => {}
            }
            continue;
        }
        result.push(ch);
    }
    result
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
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd
    }
}

pub(super) fn should_fallback_from_empty_clipboard(
    command: &str,
    has_explicit_dictation_key: bool,
) -> bool {
    !has_explicit_dictation_key && !command.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        run_dictation_command, should_fallback_from_empty_clipboard, transcript_from_command_output,
    };

    #[tokio::test]
    async fn dictation_command_trims_trailing_newlines() {
        let text = run_dictation_command("printf 'hello from test\\n'", 5)
            .await
            .expect("dictation command should succeed");
        assert_eq!(text, "hello from test");
    }

    #[test]
    fn empty_clipboard_only_falls_back_when_dictation_is_configured_without_hotkey() {
        assert!(should_fallback_from_empty_clipboard(
            "~/.local/bin/live-transcribe",
            false,
        ));
        assert!(!should_fallback_from_empty_clipboard("", false));
        assert!(!should_fallback_from_empty_clipboard(
            "~/.local/bin/live-transcribe",
            true,
        ));
    }

    #[test]
    fn transcript_from_output_strips_live_transcribe_status_lines() {
        let output = concat!(
            "\x1b[2mLoading WebRTC VAD...\x1b[0m\n",
            "\x1b[96m🎤 Live transcription started (Ctrl+C to stop)\x1b[0m\n",
            "\x1b[2mListening...\x1b[0m\n",
            "\x1b[2m[17:00:00]\x1b[0m \x1b[93m[EN]\x1b[0m \x1b[96mhello world\x1b[0m\n",
            "\x1b[2m[17:00:03]\x1b[0m \x1b[93m[ZH]\x1b[0m \x1b[92m你好\x1b[0m\n",
            "           \x1b[2m拼 nǐ hǎo\x1b[0m\n",
            "           \x1b[3m\x1b[95m→ hello\x1b[0m\n",
            "==================================================\n",
            "\x1b[96mTranscription stopped.\x1b[0m\n"
        );

        assert_eq!(
            transcript_from_command_output(output).as_deref(),
            Some("hello world hello")
        );
    }
}
