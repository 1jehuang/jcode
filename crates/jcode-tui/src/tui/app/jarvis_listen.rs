//! Continuous voice remains client-side. Only gated requests become user turns.
use super::jarvis_status::CaptureStatus;
use super::*;
use crate::tui::backend::RemoteConnection;
use jcode_voice::continuous::{ContinuousConfig, ContinuousEvent, ContinuousRecording, VoiceState};
use serde_json::json;
use std::collections::VecDeque;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jarvis_listening_keeps_one_bounded_request_and_deduplicates() {
        let mut listener = AlwaysListening::default();
        for invalid in [String::new(), "a".repeat(4097), "nul\0text".into()] {
            listener.queue_request(invalid);
            assert!(listener.pending.is_none());
        }
        listener.queue_request("show my screen".into());
        listener.queue_request("replace with different work".into());
        assert_eq!(listener.pending.as_ref().unwrap().0, "show my screen");
        listener.pending = None;
        listener.queue_request("show my screen".into());
        assert!(listener.pending.is_none());
        listener.queue_request("explain this window".into());
        assert!(listener.pending.is_some());
        listener.detach();
        assert!(listener.pending.is_none() && listener.session.is_none());
    }
}

#[derive(Default)]
pub(super) struct AlwaysListening {
    initialized: bool,
    recording: Option<ContinuousRecording>,
    status: Option<CaptureStatus>,
    wake: Option<tokio::task::JoinHandle<()>>,
    session: Option<String>,
    pending: Option<(String, Instant)>,
    turns: VecDeque<Instant>,
    last_request: Option<(String, Instant)>,
    state: Option<VoiceState>,
    failed: bool,
}
impl Drop for AlwaysListening {
    fn drop(&mut self) {
        if let Some(task) = self.wake.take() {
            task.abort();
        }
    }
}
impl AlwaysListening {
    fn queue_request(&mut self, text: String) {
        if text.trim().is_empty()
            || text.len() > 4096
            || text.contains('\0')
            || self.pending.is_some()
        {
            return;
        }
        if self
            .last_request
            .as_ref()
            .is_some_and(|(old, at)| old == &text && at.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        self.last_request = Some((text.clone(), Instant::now()));
        self.pending = Some((text, Instant::now()));
    }

    fn initialize(&mut self) -> anyhow::Result<()> {
        self.initialized = true;
        let cfg = &crate::config::config().jarvis.listening;
        anyhow::ensure!(
            (1..=3600).contains(&cfg.max_audio_seconds_per_hour)
                && (1..=600).contains(&cfg.max_classifications_per_hour)
                && (1..=120).contains(&cfg.max_agent_turns_per_hour)
                && cfg.action_probability.is_finite()
                && (0.9..=1.0).contains(&cfg.action_probability),
            "Invalid Jarvis listening limits: audio 1..3600 sec/hour, classifications 1..600/hour, agent turns 1..120/hour, probability 0.9..1"
        );
        let status = CaptureStatus::new()?;
        let key = jcode_voice::nari_api_key()
            .ok_or_else(|| anyhow::anyhow!("Always-on Jarvis requires a Nari API key"))?;
        // Resolve before opening the microphone. A missing gate must not turn
        // continuous speech into unchecked agent requests.
        crate::jev::JevClient::for_voice()?;
        let recording = ContinuousRecording::start_auto(
            Arc::new(AtomicBool::new(false)),
            &key,
            &crate::config::config().dictation.recorder,
            ContinuousConfig {
                require_jev: false,
                max_audio_seconds_per_hour: cfg.max_audio_seconds_per_hour as u32,
                max_classifications_per_hour: cfg.max_classifications_per_hour,
                action_probability: cfg.action_probability,
            },
        )?;
        self.status = Some(status);
        self.recording = Some(recording);
        self.wake = Some(tokio::spawn(async {
            let mut tick = tokio::time::interval(Duration::from_millis(50));
            loop {
                tick.tick().await;
                Bus::global().publish(BusEvent::VoiceInputWake);
            }
        }));
        Ok(())
    }

    pub(super) fn suppress_speech(&self) {
        if let Some(recording) = &self.recording {
            recording.set_suppressed(true);
        }
    }

    pub(super) fn detach(&mut self) {
        self.pending = None;
        self.session = None;
        if let Some(recording) = &self.recording {
            recording.set_suppressed(true);
            recording.set_paused(true);
        }
    }
}

impl App {
    pub(super) fn toggle_always_listening(&mut self) {
        if let Some(status) = &self.jarvis_listener.status {
            let paused = !status.paused();
            match status.set_paused(paused) {
                Ok(()) => self.set_status_notice(if paused {
                    "Jarvis microphone paused"
                } else {
                    "Jarvis microphone resuming"
                }),
                Err(error) => self.push_display_message(DisplayMessage::error(format!(
                    "Could not toggle Jarvis microphone: {error}"
                ))),
            }
        } else {
            self.set_status_notice("Always-on voice is not ready. Check the startup error.");
        }
    }

    pub(super) async fn poll_jarvis_listening(&mut self, remote: &mut RemoteConnection) -> bool {
        if !crate::tui::jarvis::always_on() {
            return false;
        }
        if !remote.jarvis_tools_configured
            || !remote.has_loaded_history()
            || self.remote_session_id.is_none()
        {
            self.jarvis_listener.detach();
            return false;
        }
        let mut listener = std::mem::take(&mut self.jarvis_listener);
        let mut changed = false;
        if !listener.initialized {
            match listener.initialize() {
                Ok(()) => self.push_display_message(DisplayMessage::system(
                    "Always-on Jarvis: local speech detection, Nari transcription, and Jev request gating. Ambient transcripts stay out of agent history. Use /voice or click the Waybar microphone to pause. Speech audio is sent to Nari; request candidates are sent to Jev.")),
                Err(error) => {
                    listener.failed = true;
                    self.push_display_message(DisplayMessage::error(format!("Always-on Jarvis did not start: {error}")));
                }
            }
            changed = true;
        }
        let session = self.remote_session_id.clone();
        if listener.session != session {
            listener.pending = None;
            listener.session = session;
            if let Some(recording) = &listener.recording {
                recording.set_suppressed(true);
            }
        }
        let paused = listener.status.as_ref().is_some_and(CaptureStatus::paused);
        let speech_busy = self
            .jarvis_speech
            .as_ref()
            .is_some_and(|player| !player.is_idle());
        let external_capture = self.external_voice.as_ref().is_some_and(|(activity, at)| {
            matches!(activity, crate::protocol::VoiceActivity::Recording { .. })
                && at.elapsed() <= Duration::from_secs(3)
        });
        let mut indicator_failed = false;
        let recording = listener.recording.take();
        if let Some(recording) = &recording {
            recording.set_paused(paused || listener.failed);
            recording.set_suppressed(speech_busy || external_capture || paused || listener.failed);
            for _ in 0..32 {
                let Some(event) = recording.try_event() else {
                    break;
                };
                match event {
                    ContinuousEvent::State(state) => {
                        listener.state = Some(state);
                    }
                    ContinuousEvent::Level(_) => {}
                    ContinuousEvent::Rejected(_) => {} // Never save ambient speech in chat or logs.
                    ContinuousEvent::Transcript(text)
                        if !paused && !listener.failed && !speech_busy && !external_capture =>
                    {
                        listener.queue_request(text);
                    }
                    ContinuousEvent::Transcript(_) => {}
                    ContinuousEvent::Error(error) => {
                        listener.failed = true;
                        listener.pending = None;
                        self.push_display_message(DisplayMessage::error(format!("Always-on Jarvis paused: {error}. Restart this mode after resolving the error.")));
                        changed = true;
                    }
                    ContinuousEvent::Stopped => {
                        listener.failed = true;
                        listener.pending = None;
                    }
                }
            }
            if let Some(status) = &mut listener.status {
                let capturing = recording.is_capturing();
                let state = if listener.failed {
                    "error"
                } else if paused {
                    "paused"
                } else if speech_busy {
                    "speech suppressed"
                } else {
                    match listener.state {
                        Some(VoiceState::Transcribing) => "transcribing",
                        Some(VoiceState::Speaking) => "hearing speech",
                        Some(VoiceState::Listening) => "listening",
                        Some(VoiceState::BudgetLimited) => "hourly speech-processing limit reached",
                        _ => "starting",
                    }
                };
                if let Err(error) = status.publish(
                    capturing,
                    state,
                    recording.audio_level(),
                    json!({"agent_turns_last_hour":listener.turns.len()}),
                ) {
                    recording.stop();
                    listener.failed = true;
                    indicator_failed = true;
                    self.push_display_message(DisplayMessage::error(format!(
                        "Capture indicator failed; microphone paused: {error}"
                    )));
                    changed = true;
                }
            }
        }
        listener.recording = recording;
        if indicator_failed {
            listener.status = None;
        }
        if paused {
            listener.pending = None;
        }
        if listener
            .pending
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(30))
        {
            listener.pending = None;
            self.set_status_notice("An old voice request expired while Jarvis was busy");
        }
        while listener
            .turns
            .front()
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(3600))
        {
            listener.turns.pop_front();
        }
        if !self.is_processing
            && self.pending_login.is_none()
            && !self.cloud_handoff_blocks_work()
            && !speech_busy
            && !paused
            && !listener.failed
            && let Some((text, _)) = listener.pending.take()
        {
            if listener.turns.len()
                >= crate::config::config()
                    .jarvis
                    .listening
                    .max_agent_turns_per_hour as usize
            {
                self.set_status_notice(
                    "Jarvis agent-turn hourly limit reached; voice request ignored",
                );
            } else {
                // Count attempts, even if delivery fails, so errors cannot create
                // an automatic retry loop or an unbounded billing path.
                listener.turns.push_back(Instant::now());
                if let Err(error) =
                    super::remote::submit_remote_voice_transcript(self, remote, &text).await
                {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Voice request was not sent: {error}"
                    )));
                }
            }
            changed = true;
        }
        self.jarvis_listener = listener;
        changed
    }
}
