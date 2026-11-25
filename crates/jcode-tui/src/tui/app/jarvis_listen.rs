//! Continuous voice remains client-side. Only gated requests become user turns.
mod dispatch;
mod history_control;
use super::jarvis_status::CaptureStatus;
use super::*;
use crate::tui::JarvisCaption;
use crate::tui::backend::RemoteConnection;
use jcode_voice::continuous::{ContinuousConfig, ContinuousEvent, ContinuousRecording, VoiceState};
use serde_json::json;
use std::collections::VecDeque;

/// Audible cue that speech reached Jarvis (gate or Fn tap). Fire and forget.
fn play_trigger_sound() -> Option<std::process::Child> {
    let sound = crate::config::config()
        .jarvis
        .listening
        .trigger_sound
        .clone();
    if sound.is_empty() || !std::path::Path::new(&sound).is_file() {
        return None;
    }
    for player in ["pw-play", "paplay"] {
        if let Ok(child) = std::process::Command::new(player)
            .arg(&sound)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            jcode_voice::timing::mark("trigger sound started");
            return Some(child);
        }
    }
    None
}

fn envelope_trigger(text: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        background_transcript: Vec<String>,
        trigger: String,
        #[serde(default)]
        #[allow(dead_code)]
        still_speaking: bool,
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
            let trigger = m.is_jarvis_user().then(|| m.user_row_text(false));
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

fn turn_retry_after(turns: &VecDeque<Instant>, limit: u32, continuation: bool) -> Option<Duration> {
    if continuation || limit == 0 {
        return None;
    }
    let oldest_blocking = turns.get(turns.len().checked_sub(limit as usize)?)?;
    Some(Duration::from_secs(3600).saturating_sub(oldest_blocking.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_turn_limit_applies_only_to_new_requests() {
        let turns = VecDeque::from([Instant::now() - Duration::from_secs(3500)]);
        assert_eq!(turn_retry_after(&turns, 0, false), None);
        assert_eq!(turn_retry_after(&turns, 2, false), None);
        assert_eq!(turn_retry_after(&turns, 1, true), None);
        let delay = turn_retry_after(&turns, 1, false).unwrap();
        assert!((99..=100).contains(&delay.as_secs()));
    }

    #[test]
    fn spoken_cue_waits_for_final_and_yields_to_ready_reply() {
        let mut playback = DeferredPlayback::default();
        playback.trigger(1, r#"{"still_speaking":true,"trigger":"open"}"#);
        playback.prepare_spoken_cue();
        assert!(playback.speech.is_none());
        playback.finalize(1, "open it");
        playback.prepare_spoken_cue();
        assert_eq!(playback.speech.take().unwrap().0, "On it");
        playback.prepare_spoken_cue();
        assert!(playback.speech.is_none());
        playback.trigger(2, "finished request");
        playback.speech = Some(("Already done".into(), Instant::now()));
        playback.prepare_spoken_cue();
        assert_eq!(playback.speech.take().unwrap().0, "Already done");
        assert!(!playback.ding);
    }

    #[test]
    fn deferred_playback_typed_reply_survives_capture_failure() {
        let mut listener = AlwaysListening::default();
        listener.defer_speech("typed reply".into());
        listener.capture_failure(None);
        assert_eq!(listener.playback.speech.as_ref().unwrap().0, "typed reply");
    }

    #[test]
    fn deferred_playback_only_matching_final_releases_or_revises() {
        let mut listener = AlwaysListening::default();
        let partial = r#"{"trigger":"open it","still_speaking":true}"#;
        listener.playback.trigger(5, partial);
        assert!(!listener.playback.ding, "no cue while still speaking");
        listener.defer_speech("opening".into());
        listener.playback.finalize(4, "different turn");
        assert!(!listener.playback.ding);
        assert!(listener.playback.partial.is_some() && listener.playback.speech.is_some());
        listener.playback.finalize(5, "open it tomorrow");
        assert!(listener.playback.ding, "cue plays once speech ends");
        assert!(listener.playback.partial.is_none() && listener.playback.speech.is_none());
        listener.playback.trigger(6, partial);
        listener.defer_speech("opening now".into());
        listener.playback.finalize(6, "open it");
        assert!(listener.playback.partial.is_none() && listener.playback.speech.is_some());
        listener.detach();
        assert!(!listener.playback.ding && listener.playback.speech.is_none());
    }

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
        assert!(listener.dispatch.pop().is_none());
        listener.detach();
        assert!(listener.captions.is_empty());
    }

    #[test]
    fn final_caption_revises_only_matching_visible_row() {
        let mut listener = AlwaysListening::default();
        listener.update_caption(1, "turn");
        listener.score_caption(1, 0.98, true);
        listener.update_caption(2, "ambient");
        listener.update_caption(1, "turn off");
        listener.finalize_caption(9, "dropped");
        listener.finalize_caption(1, "turn on");
        listener.finalize_caption(1, "turn on");
        assert_eq!(listener.captions.len(), 2);
        assert_eq!(listener.captions[0].text, "turn on");
        assert_eq!(listener.captions[0].score, Some((0.98, true)));
        assert_eq!(listener.captions[1].text, "ambient");
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
            DisplayMessage::jarvis_user(
                r#"{"background_transcript":["private"],"trigger":"question"}"#,
            ),
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
    fn hard_error_explains_deliberate_voice_retry_without_replaying_or_resetting_turns() {
        let mut app = super::super::tests::create_test_app();
        let listener = &mut app.jarvis_listener;
        listener.initialized = true;
        listener.turns.push_back(Instant::now());
        listener.update_caption(7, "private caption");
        listener.playback.trigger(7, "private caption");
        let notice = listener
            .capture_failure(Some(&jcode_voice::VoiceError::NariCreditsExhausted))
            .unwrap();
        assert!(notice.contains("Add credits") && notice.contains("/voice"));
        assert!(!notice.contains("private"));
        assert!(listener.failed && listener.initialized);
        assert!(listener.capture_failure(None).is_none()); // subsequent Stopped must not mask the cause
        app.toggle_always_listening();
        assert!(app.status_notice.as_ref().unwrap().0.contains("Retrying"));
        let listener = &app.jarvis_listener;
        assert!(!listener.initialized && !listener.failed);
        assert_eq!(listener.turns.len(), 1);
        assert!(listener.captions.is_empty() && !listener.playback.ding);
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
}

#[derive(Default)]
pub(super) struct AlwaysListening {
    playback: DeferredPlayback,
    history: history_control::HistoryControl,
    history_command: Option<String>,
    history_request: Option<u64>,
    initialized: bool,
    recording: Option<ContinuousRecording>,
    status: Option<CaptureStatus>,
    wake: Option<tokio::task::JoinHandle<()>>,
    session: Option<String>,
    dispatch: dispatch::Dispatch,
    turns: VecDeque<Instant>,
    state: Option<VoiceState>,
    failed: bool,
    captions: VecDeque<JarvisCaption>,
    chat_version: Option<u64>,
    triggered_at: Option<Instant>,
}
#[derive(Default)]
struct DeferredPlayback {
    partial: Option<(u64, String, Instant)>,
    cue_player: Option<(std::process::Child, Instant)>,
    latest: Option<u64>,
    speech: Option<(String, Instant)>,
    ding: bool,
}

impl DeferredPlayback {
    fn prepare_spoken_cue(&mut self) {
        if std::mem::take(&mut self.ding) && self.speech.is_none() {
            self.speech = Some(("On it".into(), Instant::now()));
        }
    }

    fn trigger(&mut self, id: u64, text: &str) {
        self.latest = Some(id);
        self.speech = None;
        self.partial = match serde_json::from_str::<serde_json::Value>(text) {
            Ok(env) if env["still_speaking"].as_bool() == Some(true) => Some((
                id,
                env["trigger"].as_str().unwrap_or("").trim().to_owned(),
                Instant::now(),
            )),
            _ => None,
        };
        // No cue while the user is still talking: it plays on finalize.
        self.ding = self.partial.is_none();
    }

    fn finalize(&mut self, id: u64, text: &str) {
        let Some((_, partial, _)) = self.partial.take_if(|(old, _, _)| *old == id) else {
            return;
        };
        self.ding = true;
        if partial != text.trim() {
            self.speech = None;
        }
    }
}
fn stop_cue(child: &mut std::process::Child) {
    if let Err(error) = child.kill()
        && error.kind() != std::io::ErrorKind::InvalidInput
    {
        crate::logging::warn(&format!("Could not stop voice cue: {error}"));
    }
}
impl Drop for DeferredPlayback {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.cue_player.take() {
            stop_cue(&mut child);
            if let Err(error) = child.wait() {
                crate::logging::warn(&format!("Could not reap voice cue: {error}"));
            }
        }
    }
}
impl Drop for AlwaysListening {
    fn drop(&mut self) {
        if let Some(task) = self.wake.take() {
            task.abort();
        }
    }
}
impl AlwaysListening {
    pub(super) fn queue_history(&mut self, cmd: &str) -> &'static str {
        if self.recording.is_none() || self.session.is_none() {
            return "Always-on Jarvis is not ready";
        }
        self.history_command = Some(
            if cmd.trim().is_empty() {
                "status"
            } else {
                cmd.trim()
            }
            .into(),
        );
        "Local audio history command queued"
    }
    fn playback_trace(&self) -> jcode_voice::speech::PlaybackTrace {
        jcode_voice::speech::PlaybackTrace {
            session: self.session.clone().unwrap_or_else(|| "none".into()),
            utterance: self.playback.latest.unwrap_or(0),
        }
    }
    pub(super) fn defer_speech(&mut self, text: String) {
        self.playback_trace().mark("deferred", 0);
        self.playback.speech = Some((text, Instant::now()));
        jcode_voice::timing::mark("speech callback deferred");
    }

    pub(super) fn clear_playback(&mut self) {
        self.playback = DeferredPlayback::default();
    }
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
        if let Some(row) = self.captions.iter_mut().find(|r| r.id == id) {
            // Streaming revision of the same utterance: grow in place.
            row.text = text;
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

    fn finalize_caption(&mut self, id: u64, text: &str) {
        // A late final must not resurrect an evicted or suppressed row.
        if self.captions.iter().any(|r| r.id == id) {
            self.update_caption(id, text);
        }
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
                _ => "starting",
            }
        }
    }

    fn trace(&self, id: u64, stage: &str) {
        dispatch::trace(self.session.as_deref(), id, stage);
    }
    fn clear_dispatch(&mut self, reason: &str) {
        let session = self.session.as_deref();
        self.dispatch
            .clear(|id| dispatch::trace(session, id, reason));
    }
    fn capture_failure(&mut self, error: Option<&jcode_voice::VoiceError>) -> Option<String> {
        if self.failed {
            return None;
        }
        self.failed = true;
        if let Some(status) = &mut self.status {
            status.set_failure(error);
        }
        self.clear_dispatch("capture_failed");
        if self.playback.latest.is_some() {
            self.clear_playback();
        }
        self.captions.clear();
        self.triggered_at = None;
        let reason = error
            .map(ToString::to_string)
            .unwrap_or_else(|| "Capture stopped".into());
        Some(format!(
            "Always-on Jarvis stopped: {reason}. After resolving the issue, use /voice to retry."
        ))
    }

    fn request_retry(&mut self) -> &'static str {
        if self.recording.as_ref().is_some_and(|r| !r.is_finished()) {
            return "Microphone is still stopping. Use /voice again once stopped.";
        }
        self.detach();
        self.recording = None;
        self.status = None;
        if let Some(task) = self.wake.take() {
            task.abort();
        }
        self.state = None;
        self.failed = false;
        self.initialized = false;
        Bus::global().publish(BusEvent::VoiceInputWake);
        "Retrying always-on Jarvis. Previous speech will not be replayed."
    }

    fn initialize(&mut self) -> anyhow::Result<()> {
        self.initialized = true;
        self.status = Some(CaptureStatus::new()?);
        self.wake = Some(tokio::spawn(async {
            let mut tick = tokio::time::interval(Duration::from_millis(50));
            loop {
                tick.tick().await;
                Bus::global().publish(BusEvent::VoiceInputWake);
            }
        }));
        let cfg = &crate::config::config().jarvis.listening;
        anyhow::ensure!(
            cfg.max_agent_turns_per_hour <= 120
                && cfg.action_probability.is_finite()
                && (0.5..=1.0).contains(&cfg.action_probability),
            "Invalid Jarvis listening limits: agent turns 0 (unlimited) or 1..120/hour, probability 0.5..1"
        );
        let key = jcode_voice::nari_api_key().ok_or(jcode_voice::VoiceError::NariNotConfigured)?;
        // Resolve before opening the microphone. A missing gate must not turn
        // continuous speech into unchecked agent requests.
        crate::jev::JevClient::for_voice()?;
        let recording = ContinuousRecording::start_auto_with_restoration(
            Arc::new(AtomicBool::new(false)),
            &key,
            &crate::config::config().dictation.recorder,
            ContinuousConfig {
                require_jev: false,
                action_probability: cfg.action_probability,
                live_action_probability: Some(|| {
                    crate::config::config().jarvis.listening.action_probability
                }),
                warm_nari: cfg.warm_nari,
            },
            || Box::pin(crate::network_retry::wait_for_connectivity_restoration()),
        )?;
        self.recording = Some(recording);
        Ok(())
    }

    pub(super) fn suppress_speech(&self) {
        if let Some(recording) = &self.recording {
            recording.set_suppressed(true);
        }
    }

    pub(super) fn detach(&mut self) {
        self.clear_playback();
        self.captions.clear();
        self.clear_dispatch("detached");
        self.chat_version = None;
        self.triggered_at = None;
        self.session = None;
        self.history = Default::default();
        self.history_command = None;
        self.history_request = None;
        if let Some(recording) = &self.recording {
            recording.set_suppressed(true);
            recording.set_paused(true);
            recording.reset_audio_history(Some(false));
        }
    }
}

impl App {
    pub(super) fn toggle_always_listening(&mut self) {
        if self.jarvis_listener.failed {
            let notice = self.jarvis_listener.request_retry();
            self.set_status_notice(notice);
        } else if let Some(status) = &self.jarvis_listener.status {
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
        if listener.failed
            && listener
                .status
                .as_ref()
                .is_some_and(CaptureStatus::take_retry)
        {
            self.set_status_notice(listener.request_retry());
            changed = true;
        }
        if !listener.initialized {
            match listener.initialize() {
                Ok(()) => self.push_display_message(DisplayMessage::system(
                    "Always-on Jarvis: local speech detection, Nari transcription, and Jev request gating. Recent speech and visible chat are sent to Jev. Only accepted requests and their preceding speech are sent to the agent. Use /voice or click the Waybar microphone to pause. Speech audio is sent to Nari; request candidates are sent to Jev.")),
                Err(error) => {
                    listener.failed = true;
                    if let Some(status) = &mut listener.status { status.set_failure(error.downcast_ref::<jcode_voice::VoiceError>()); }
                    self.push_display_message(DisplayMessage::error(format!("Always-on Jarvis did not start: {error}. After resolving the issue, use /voice to retry.")));
                }
            }
            changed = true;
        }
        let session = self.remote_session_id.clone();
        if listener.session != session {
            listener.detach();
            listener.session = session;
            #[cfg(unix)]
            if let Some(status) = &listener.status {
                listener.history.attach(status.private_directory());
            }
        }
        let paused = listener.status.as_ref().is_some_and(CaptureStatus::paused);
        let cue_busy = listener
            .playback
            .cue_player
            .as_mut()
            .is_some_and(|(child, at)| {
                if at.elapsed() > Duration::from_secs(3) {
                    stop_cue(child);
                }
                matches!(child.try_wait(), Ok(None))
            });
        if !cue_busy {
            listener.playback.cue_player = None;
        }
        let mut speech_busy = cue_busy
            || self
                .jarvis_speech
                .as_ref()
                .is_some_and(|player| player.owns_audio_output());
        let external_capture = self.external_voice.as_ref().is_some_and(|(activity, at)| {
            matches!(activity, crate::protocol::VoiceActivity::Recording { .. })
                && at.elapsed() <= Duration::from_secs(3)
        }) || listener
            .status
            .as_ref()
            .is_some_and(CaptureStatus::external_dictation_active);
        let mut indicator_failed = false;
        let recording = listener.recording.take();
        if listener.failed
            && recording.is_none()
            && let Some(status) = &mut listener.status
        {
            // Setup can fail before a worker exists. Still heartbeat the error indicator
            // so its owner can explicitly retry without opening a microphone on its own.
            if status.publish(false, "error", 0.0, json!({})).is_err() {
                indicator_failed = true;
            }
        }
        if let Some(recording) = &recording {
            if let Some(cmd) = listener
                .history_command
                .take()
                .or_else(|| listener.history.incoming())
            {
                let blocked = speech_busy
                    || external_capture
                    || listener.failed
                    || listener.playback.partial.is_some()
                    || self.voice_input.is_some();
                let notice =
                    listener
                        .history
                        .command(recording, &cmd, listener.history_request, blocked);
                if let Some(caption) = listener.history.caption.take() {
                    self.set_status_notice(format!("Replaying local audio: {caption}"));
                } else {
                    self.set_status_notice(notice);
                }
            }
            speech_busy |= listener.history.busy();
            recording.set_paused(paused || listener.failed);
            // Fn tap (companion): act on what was just heard.
            if listener
                .status
                .as_ref()
                .is_some_and(CaptureStatus::take_wake)
            {
                if paused || listener.failed {
                    // Say why the tap did nothing instead of dropping it.
                    self.set_status_notice("Jarvis is paused; Fn tap ignored");
                } else if recording.manual_trigger() {
                    self.set_status_notice("Jarvis: sending what it just heard");
                } else {
                    self.set_status_notice("Jarvis hasn't heard anything recently");
                }
                changed = true;
            }
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
                    ContinuousEvent::Recovering { error, delay } => {
                        listener.clear_dispatch("capture_recovering");
                        if listener.playback.latest.is_some() {
                            listener.clear_playback();
                            if let Some(player) = &self.jarvis_speech {
                                player.cancel();
                            }
                        }
                        self.set_status_notice(format!("{error}. Microphone released for {}s. Repeat the request when listening resumes.", delay.as_secs()));
                        changed = true;
                    }
                    ContinuousEvent::Rejected(_) => {} // Never save ambient speech in chat or logs.
                    ContinuousEvent::Transcript { id, text }
                        if !paused && !listener.failed && !speech_busy && !external_capture =>
                    {
                        listener.triggered_at = Some(Instant::now());
                        let outcome = listener.dispatch.enqueue(id, text.clone());
                        if outcome == "queued" {
                            if let Some(player) = &self.jarvis_speech {
                                player.cancel();
                            }
                            listener.history_request = Some(id);
                            listener.playback.trigger(id, &text);
                        }
                        listener.trace(id, outcome);
                        changed = true;
                    }
                    ContinuousEvent::Transcript { id, .. } => listener.trace(id, "suppressed"),
                    ContinuousEvent::Continuation { id, text }
                        if !paused && !listener.failed && !external_capture =>
                    {
                        listener.finalize_caption(id, &text);
                        listener.playback.finalize(id, &text);
                        let outcome = listener.dispatch.finalize(id, text);
                        listener.trace(id, outcome);
                        changed = true;
                    }
                    ContinuousEvent::Continuation { id, .. } => {
                        listener.trace(id, "final_suppressed")
                    }
                    ContinuousEvent::Error(error) => {
                        let voice_reply = listener.playback.latest.is_some();
                        if let Some(notice) = listener.capture_failure(Some(&error)) {
                            if voice_reply && let Some(player) = &self.jarvis_speech {
                                player.cancel();
                            }
                            self.push_display_message(DisplayMessage::error(notice));
                        }
                        changed = true;
                    }
                    ContinuousEvent::Stopped => {
                        let voice_reply = listener.playback.latest.is_some();
                        if let Some(notice) = listener.capture_failure(None) {
                            if voice_reply && let Some(player) = &self.jarvis_speech {
                                player.cancel();
                            }
                            self.push_display_message(DisplayMessage::error(notice));
                            changed = true;
                        }
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
            let state = if push_to_talk_level.is_some() && !listener.failed {
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
                    json!({"agent_turns_last_hour":listener.turns.len(),"audio_history":listener.history.info(recording)}),
                ) {
                    recording.stop();
                    listener.failed = true;
                    indicator_failed = true;
                    self.push_display_message(DisplayMessage::error(format!(
                        "Capture indicator failed; microphone paused: {error}. Use /voice to retry."
                    )));
                    changed = true;
                }
            }
        }
        if paused || listener.failed {
            changed |= !listener.captions.is_empty();
            listener.captions.clear();
            listener.clear_dispatch("paused_or_failed");
            listener.chat_version = None;
        }
        listener.recording = recording;
        if indicator_failed {
            listener.status = None;
        }
        let sid = listener.session.as_deref();
        listener
            .dispatch
            .expire(|id| dispatch::trace(sid, id, "expired"));
        while listener
            .turns
            .front()
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(3600))
        {
            listener.turns.pop_front();
        }
        if self.pending_login.is_none()
            && !self.cloud_handoff_blocks_work()
            && !speech_busy
            && !paused
            && !listener.failed
            && let Some(req) = listener.dispatch.pop()
        {
            let limit = crate::config::config()
                .jarvis
                .listening
                .max_agent_turns_per_hour;
            if let Some(delay) = turn_retry_after(&listener.turns, limit, req.continuation) {
                listener.trace(req.id, "hourly_limit");
                self.set_status_notice(format!(
                    "Jarvis voice request not sent: {limit}-turn hourly cap reached. Try again in {}s.",
                    delay.as_secs_f64().ceil() as u64
                ));
            } else {
                let interleave = self.is_processing;
                let result = dispatch::submit(self, remote, &req.text, interleave).await;
                // The finished utterance must not wait for the turn's next safe
                // point: cancel the provisional turn. The server keeps the queued
                // interrupt and runs it as the next turn (Esc-redirect path).
                let result = match result {
                    Ok(id) if interleave && req.continuation => remote
                        .cancel_with_reason("jarvis_final_transcript")
                        .await
                        .map(|_| {
                            self.remote_interrupt_ack_deadline =
                                Some(Instant::now() + Duration::from_secs(3));
                            id
                        }),
                    other => other,
                };
                let (request_id, stage) = match result {
                    Err(error) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Voice request was not sent: {error}"
                        )));
                        (None, "send_failed")
                    }
                    Ok(id) => {
                        if !req.continuation {
                            listener.turns.push_back(Instant::now());
                        }
                        listener.dispatch.sent(&req);
                        (
                            Some(id),
                            if interleave {
                                "interleave_sent"
                            } else {
                                "sent"
                            },
                        )
                    }
                };
                crate::logging::info(&format!(
                    "JARVIS_DISPATCH session={} utterance={} stage={} monotonic_ms={} request_id={request_id:?} queue_ms={} provisional={} final_update={}",
                    listener.session.as_deref().unwrap_or("none"),
                    req.id,
                    stage,
                    jcode_voice::timing::monotonic_ms(),
                    req.at.elapsed().as_millis(),
                    req.provisional,
                    req.continuation
                ));
            }
            changed = true;
        }
        let orphaned_partial = listener
            .playback
            .partial
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() >= Duration::from_secs(180));
        let disabled = paused || listener.failed;
        if (disabled && listener.playback.latest.is_some())
            || external_capture
            || self.voice_input.is_some()
            || orphaned_partial
        {
            if let Some(player) = &self.jarvis_speech {
                player.cancel();
            }
            listener.clear_playback();
        } else if !speech_busy && listener.playback.partial.is_none() {
            if listener.playback.ding && crate::tui::jarvis::handterm_spoken_cue() {
                listener.playback.prepare_spoken_cue();
            }
            let claim = || {
                if disabled {
                    return !listener
                        .recording
                        .as_ref()
                        .is_some_and(ContinuousRecording::is_capturing);
                }
                listener.state == Some(VoiceState::Listening)
                    && listener
                        .recording
                        .as_ref()
                        .is_some_and(ContinuousRecording::suppress_if_quiet)
            };
            if let Some((text, at)) = listener
                .playback
                .speech
                .take()
                .filter(|(_, at)| at.elapsed() < Duration::from_secs(30))
            {
                if let Some(player) = &self.jarvis_speech {
                    player.cancel(); // replace any synthesized but not yet authorized reply
                    listener.playback.ding = false;
                    let trace = listener.playback_trace();
                    trace.mark("enqueue", at.elapsed().as_millis());
                    if let Err(error) = player.enqueue_gated(&text, trace) {
                        self.push_display_message(DisplayMessage::error(error.to_string()));
                        changed = true;
                    }
                }
            } else if self
                .jarvis_speech
                .as_ref()
                .is_some_and(|p| p.authorize_ready(claim))
            {
                listener.playback_trace().mark("quiet_authorized", 0);
            } else if listener.playback.ding
                && self.jarvis_speech.as_ref().is_none_or(|p| p.is_idle())
                && claim()
            {
                listener.playback.ding = false;
                listener.playback.cue_player = play_trigger_sound().map(|c| {
                    listener.playback_trace().mark("ding_started", 0);
                    (c, Instant::now())
                });
            }
        }
        self.jarvis_listener = listener;
        changed
    }
}
