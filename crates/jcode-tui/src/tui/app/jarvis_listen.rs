//! Continuous voice remains client-side. Only gated requests become user turns.
use super::jarvis_status::CaptureStatus;
use super::*;
use crate::tui::JarvisCaption;
use crate::tui::backend::RemoteConnection;
use jcode_voice::continuous::{ContinuousConfig, ContinuousEvent, ContinuousRecording, VoiceState};
use serde_json::json;
use std::collections::VecDeque;

fn envelope_trigger(text: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        background_transcript: Vec<String>,
        trigger: String,
    }
    if text.len() > 128 * 1024 {
        return None;
    }
    // Ordinary chat text is not an envelope and is displayed unchanged.
    let Ok(e) = serde_json::from_str::<Envelope>(text) else {
        return None;
    };
    (e.background_transcript.len() <= 16
        && e.background_transcript
            .iter()
            .all(|s| s.chars().count() <= 1024)
        && !e.trigger.trim().is_empty()
        && !e.trigger.contains('\0')
        && e.trigger.len() <= 4096)
        .then_some(e.trigger)
}

fn recent_chat(messages: &[DisplayMessage]) -> Vec<(String, String)> {
    let mut chat: Vec<_> = messages
        .iter()
        .rev()
        .filter(|m| matches!(m.role.as_str(), "user" | "assistant") && !m.content.trim().is_empty())
        .filter_map(|m| {
            let visible;
            let text = if m.role == "assistant" {
                visible = super::input::strip_reasoning_lines(&m.content);
                visible.as_str()
            } else {
                m.content.as_str()
            };
            if text.trim().is_empty() {
                return None;
            }
            let (text, _) = jcode_session_types::strip_transcription(text);
            let trigger = if m.role == "user" {
                envelope_trigger(&text)
            } else {
                None
            };
            Some((
                m.role.clone(),
                trigger
                    .as_deref()
                    .unwrap_or(&text)
                    .chars()
                    .take(1024)
                    .collect(),
            ))
        })
        .take(8)
        .collect();
    chat.reverse();
    chat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_is_bounded_terminal_safe_and_never_queued() {
        let mut listener = AlwaysListening::default();
        listener.update_caption(1, "\x1b[31mhello\x1b[0m\nworld\0\u{202e}");
        assert_eq!(
            listener.captions.front().map(|r| r.text.as_str()),
            Some("hello world")
        );
        listener.update_caption(2, &"界".repeat(500));
        assert_eq!(listener.captions.back().unwrap().text.chars().count(), 160);
        assert!(listener.pending.is_none() && listener.last_request.is_none());
        listener.detach();
        assert!(listener.captions.is_empty());
    }

    #[test]
    fn caption_scores_follow_ids_not_repeated_text_or_position() {
        let mut listener = AlwaysListening::default();
        for id in 1..=5 {
            listener.update_caption(id, "again");
        }
        assert!(!listener.score_caption(1, 0.99, true));
        assert!(listener.score_caption(4, 0.99, true));
        assert!(listener.score_caption(2, 0.25, false));
        let scores: Vec<_> = listener.captions.iter().map(|r| r.score).collect();
        assert_eq!(
            scores,
            [Some((0.25, false)), None, Some((0.99, true)), None]
        );
    }

    #[test]
    fn context_only_includes_visible_dialogue_and_unwraps_voice() {
        let rows = [
            DisplayMessage::system("secret"),
            DisplayMessage::user(r#"{"background_transcript":["private"],"trigger":"question"}"#),
            DisplayMessage::assistant(format!(
                "{}\nreply",
                jcode_tui_markdown::reasoning_line_markup("secret")
            )),
        ];
        assert_eq!(
            recent_chat(&rows),
            vec![
                ("user".into(), "question".into()),
                ("assistant".into(), "reply".into())
            ]
        );
    }

    #[test]
    fn trigger_expires_without_overriding_pause_or_failure() {
        let now = Instant::now();
        let mut listener = AlwaysListening::default();
        listener.triggered_at = Some(now);
        listener.state = Some(VoiceState::Listening);
        assert_eq!(listener.indicator_state(false, true, now), "triggered");
        assert_eq!(listener.indicator_state(true, false, now), "paused");
        assert_eq!(
            listener.indicator_state(false, false, now + Duration::from_secs(3)),
            "listening"
        );
        listener.failed = true;
        assert_eq!(listener.indicator_state(true, false, now), "error");
    }

    #[test]
    fn jarvis_listening_keeps_one_bounded_request_and_deduplicates() {
        let mut listener = AlwaysListening::default();
        for invalid in [String::new(), "a".repeat(4097), "nul\0text".into()] {
            listener.queue_request(invalid);
            assert!(listener.pending.is_none());
        }
        let envelope =
            |trigger, bg| json!({"trigger":trigger,"background_transcript":[bg]}).to_string();
        let original = envelope("show my screen", "a");
        listener.queue_request(original.clone());
        listener.queue_request(envelope("replace with different work", "a"));
        assert_eq!(listener.pending.as_ref().unwrap().0, original);
        listener.pending = None;
        listener.queue_request(envelope("show my screen", "b"));
        assert!(listener.pending.is_none());
        listener.queue_request(envelope("explain this window", "b"));
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
    captions: VecDeque<JarvisCaption>,
    chat_version: Option<u64>,
    triggered_at: Option<Instant>,
}
impl Drop for AlwaysListening {
    fn drop(&mut self) {
        if let Some(task) = self.wake.take() {
            task.abort();
        }
    }
}
impl AlwaysListening {
    pub(super) fn captions(&self) -> &VecDeque<JarvisCaption> {
        &self.captions
    }

    fn caption_text(text: &str) -> String {
        let plain = crate::message::strip_ansi_escape_sequences(text);
        let text: String = plain.chars().filter_map(|c| {
            if c.is_whitespace() { Some(' ') } else if c.is_control() || matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { None } else { Some(c) }
        }).take(160).collect();
        text.trim().to_owned()
    }

    fn update_caption(&mut self, id: u64, text: &str) {
        let text = Self::caption_text(text);
        if text.is_empty() {
            return;
        }
        if self.captions.len() == 4 {
            self.captions.pop_front();
        }
        self.captions.push_back(JarvisCaption {
            id,
            text,
            score: None,
        });
    }

    fn score_caption(&mut self, id: u64, probability: f64, accepted: bool) -> bool {
        let Some(row) = self.captions.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        row.score = Some((probability, accepted));
        true
    }

    fn indicator_state(&self, paused: bool, speech_busy: bool, now: Instant) -> &'static str {
        if self.failed {
            "error"
        } else if paused {
            "paused"
        } else if self
            .triggered_at
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(3))
        {
            "triggered"
        } else if speech_busy {
            "speech suppressed"
        } else {
            match self.state {
                Some(VoiceState::Transcribing) => "transcribing",
                Some(VoiceState::Speaking) => "hearing speech",
                Some(VoiceState::Listening) => "listening",
                Some(VoiceState::BudgetLimited) => "hourly speech-processing limit reached",
                _ => "starting",
            }
        }
    }

    fn queue_request(&mut self, text: String) {
        let Some(trigger) = envelope_trigger(&text) else {
            return;
        };
        if self.pending.is_some() {
            return;
        }
        if self
            .last_request
            .as_ref()
            .is_some_and(|(old, at)| old == &trigger && at.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        self.last_request = Some((trigger, Instant::now()));
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
        self.captions.clear();
        self.last_request = None;
        self.chat_version = None;
        self.triggered_at = None;
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
                    "Always-on Jarvis: local speech detection, Nari transcription, and Jev request gating. Recent speech and visible chat are sent to Jev. Only accepted requests and their preceding speech are sent to the agent. Use /voice or click the Waybar microphone to pause. Speech audio is sent to Nari; request candidates are sent to Jev.")),
                Err(error) => {
                    listener.failed = true;
                    self.push_display_message(DisplayMessage::error(format!("Always-on Jarvis did not start: {error}")));
                }
            }
            changed = true;
        }
        let session = self.remote_session_id.clone();
        if listener.session != session {
            listener.detach();
            listener.session = session;
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
            if !paused
                && !listener.failed
                && listener.chat_version != Some(self.display_messages_version)
            {
                recording.set_chat_context(recent_chat(&self.display_messages));
                listener.chat_version = Some(self.display_messages_version);
            }
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
                    ContinuousEvent::Caption { id, text } => {
                        if !paused && !listener.failed && !speech_busy && !external_capture {
                            listener.update_caption(id, &text);
                            changed = true;
                        }
                    }
                    ContinuousEvent::CaptionScore {
                        id,
                        probability,
                        accepted,
                    } => {
                        changed |= listener.score_caption(id, probability, accepted);
                    }
                    ContinuousEvent::Rejected(_) => {} // Never save ambient speech in chat or logs.
                    ContinuousEvent::Transcript(text)
                        if !paused && !listener.failed && !speech_busy && !external_capture =>
                    {
                        listener.triggered_at = Some(Instant::now());
                        listener.queue_request(text);
                        changed = true;
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
            let state = listener.indicator_state(paused, speech_busy, Instant::now());
            // Companion push-to-talk (Fn or Copilot) owns the microphone for
            // this session while Jarvis's own capture is suppressed. Show its
            // live level so the Waybar waveform keeps tracking the voice.
            let push_to_talk_level = external_capture
                .then(|| match self.external_voice.as_ref() {
                    Some((crate::protocol::VoiceActivity::Recording { levels, .. }, _)) => {
                        Some(levels.last().copied().unwrap_or(0.0))
                    }
                    _ => None,
                })
                .flatten();
            let state = if push_to_talk_level.is_some() {
                "push to talk"
            } else {
                state
            };
            if let Some(status) = &mut listener.status {
                let capturing = recording.is_capturing() || push_to_talk_level.is_some();
                if let Err(error) = status.publish(
                    capturing,
                    state,
                    recording
                        .audio_level()
                        .max(push_to_talk_level.unwrap_or(0.0)),
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
        if paused || listener.failed {
            changed |= !listener.captions.is_empty();
            listener.captions.clear();
            listener.last_request = None;
            listener.chat_version = None;
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
                self.push_display_message(DisplayMessage::user(text.clone()));
                if let Err(error) = self.begin_remote_send(remote, text, vec![], false).await {
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
