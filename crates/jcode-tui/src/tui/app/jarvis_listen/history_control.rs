//! Local one-shot control for the microphone owner. Never exposes a clip or text over IPC.
use super::*;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};
#[derive(Default)]
pub(super) struct HistoryControl {
    #[cfg(unix)]
    socket: Option<(std::os::unix::net::UnixDatagram, std::path::PathBuf)>,
    player: Option<(Child, Instant)>,
    last: &'static str,
    commands: u64,
    pub(super) caption: Option<String>,
}
impl Drop for HistoryControl {
    fn drop(&mut self) {
        self.stop();
        #[cfg(unix)]
        if let Some((_, p)) = self.socket.take()
            && let Err(err) = std::fs::remove_file(&p)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            crate::logging::debug(&format!(
                "Failed to remove Jarvis history control socket {}: {err}",
                p.display()
            ));
        }
    }
}
impl HistoryControl {
    fn stop(&mut self) {
        if let Some((mut child, _)) = self.player.take() {
            if let Err(err) = child.kill()
                && err.kind() != std::io::ErrorKind::InvalidInput
            {
                crate::logging::debug(&format!("Failed to stop Jarvis replay player: {err}"));
            }
            if let Err(err) = child.wait() {
                crate::logging::debug(&format!("Failed to reap Jarvis replay player: {err}"));
            }
        }
    }
    pub fn busy(&mut self) -> bool {
        if let Some((child, at)) = &mut self.player {
            if at.elapsed() < Duration::from_secs(25) && matches!(child.try_wait(), Ok(None)) {
                return true;
            }
            self.last = if matches!(child.try_wait(),Ok(Some(s)) if s.success()) {
                "played"
            } else {
                "player failed"
            };
            self.stop();
        }
        false
    }
    #[cfg(unix)]
    pub fn attach(&mut self, dir: &std::path::Path) {
        use std::os::unix::net::UnixDatagram;
        let path = dir.join(format!(
            "{}.history-{:016x}.sock",
            std::process::id(),
            rand::random::<u64>()
        ));
        if let Ok(s) = UnixDatagram::bind(&path) {
            if s.set_nonblocking(true).is_ok() {
                self.socket = Some((s, path));
            } else if let Err(err) = std::fs::remove_file(&path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                crate::logging::debug(&format!(
                    "Failed to remove nonblocking-failed Jarvis history socket {}: {err}",
                    path.display()
                ));
            }
        }
    }
    pub fn incoming(&self) -> Option<String> {
        #[cfg(unix)]
        {
            let mut buf = [0; 64];
            let n = match self.socket.as_ref()?.0.recv(&mut buf) {
                Ok(n) => n,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return None,
                Err(err) => {
                    crate::logging::debug(&format!(
                        "Failed to read Jarvis history control command: {err}"
                    ));
                    return None;
                }
            };
            if n < buf.len() {
                return match std::str::from_utf8(&buf[..n]) {
                    Ok(text) => Some(text.trim().to_owned()),
                    Err(_) => None,
                };
            }
        }
        None
    }
    pub fn info(&self, rec: &ContinuousRecording) -> serde_json::Value {
        let (enabled, ids) = rec.audio_history_status();
        #[cfg(unix)]
        let socket = self
            .socket
            .as_ref()
            .and_then(|(_, p)| p.file_name())
            .and_then(|s| s.to_str());
        #[cfg(not(unix))]
        let socket: Option<&str> = None;
        json!({"socket":socket,"commands":self.commands,"enabled":enabled,"ids_newest_first":ids.iter().map(u64::to_string).collect::<Vec<_>>(),"last":self.last})
    }
    pub fn command(
        &mut self,
        rec: &ContinuousRecording,
        command: &str,
        last_request: Option<u64>,
        blocked: bool,
    ) -> &'static str {
        let result = match command {
            "on" => {
                if !rec.audio_history_status().0 {
                    rec.reset_audio_history(Some(true));
                }
                "history on"
            }
            "off" | "clear" => {
                self.stop();
                rec.reset_audio_history((command == "off").then_some(false));
                if command == "off" {
                    "history off"
                } else {
                    "history cleared"
                }
            }
            "status" => {
                if rec.audio_history_status().0 {
                    "history on"
                } else {
                    "history off"
                }
            }
            _ => {
                let (_, ids) = rec.audio_history_status();
                let id = match command {
                    "replay" | "replay 1" => ids.first().copied(),
                    "replay 2" => ids.get(1).copied(),
                    "replay 3" => ids.get(2).copied(),
                    "previous" => {
                        last_request.and_then(|last| ids.iter().copied().find(|id| *id < last))
                    }
                    _ => None,
                };
                if !matches!(
                    command,
                    "replay" | "replay 1" | "replay 2" | "replay 3" | "previous"
                ) {
                    "use on, off, clear, status, replay [1-3], or previous"
                } else if blocked || self.busy() {
                    "audio busy; try again"
                } else if let Some((pcm, text)) = id.and_then(|i| rec.audio_history_clip(i)) {
                    if rec.is_capturing() && !rec.suppress_if_quiet() {
                        self.commands = self.commands.wrapping_add(1);
                        self.last = "microphone busy; try again";
                        return self.last;
                    }
                    rec.set_suppressed(true);
                    match Command::new("paplay")
                        .args(["--raw", "--format=s16le", "--channels=1", "--rate=16000"])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        Ok(mut child) => {
                            let Some(mut input) = child.stdin.take() else {
                                if let Err(err) = child.kill()
                                    && err.kind() != std::io::ErrorKind::InvalidInput
                                {
                                    crate::logging::debug(&format!(
                                        "Failed to stop Jarvis replay player without stdin: {err}"
                                    ));
                                }
                                if let Err(err) = child.wait() {
                                    crate::logging::debug(&format!(
                                        "Failed to reap Jarvis replay player without stdin: {err}"
                                    ));
                                }
                                self.commands = self.commands.wrapping_add(1);
                                self.last = "local paplay unavailable";
                                return self.last;
                            };
                            std::thread::spawn(move || {
                                for chunk in pcm.chunks(1024) {
                                    let bytes: Vec<_> =
                                        chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                                    if input.write_all(&bytes).is_err() {
                                        break;
                                    }
                                }
                            });
                            self.caption = Some(AlwaysListening::caption_text(&text));
                            self.player = Some((child, Instant::now()));
                            "playing"
                        }
                        Err(_) => "local paplay unavailable",
                    }
                } else {
                    "no matching final clip"
                }
            }
        };
        self.commands = self.commands.wrapping_add(1);
        self.last = result;
        result
    }
}
