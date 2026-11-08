//! Built-in voice input for the terminal UI, matching Jcode Desktop.
//!
//! Press the voice key (Ctrl+Space by default) or run `/voice`, speak, then
//! press again to send. On terminals that report key releases (Kitty keyboard
//! protocol), holding the key and letting go also sends, like Desktop's
//! hold-to-talk. Esc cancels. Audio streams to Nari while recording, the live
//! transcript and a level meter show in the status line, and the final text is
//! sent to the agent wrapped in `<transcription>` tags. Whatever is already
//! typed in the composer stays there, untouched.
use super::*;
use crossterm::event::KeyEvent;
use jcode_voice::{self as voice, NariEvent, NariRecording, VoiceError};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;

/// Holding the key at least this long before releasing it means push-to-talk:
/// the release sends. A shorter tap leaves recording on until the next press.
const HOLD_TO_TALK: Duration = Duration::from_millis(400);
/// Terminals without release reporting repeat a held key as fresh presses.
/// Presses closer together than this are one continuous hold, not a toggle.
const REPEAT_DEBOUNCE: Duration = Duration::from_millis(300);
/// Meter and live transcript refresh cadence while voice input is active.
const METER_TICK: Duration = Duration::from_millis(80);
const METER_BARS: usize = 10;
/// Level history kept for the composer waveform, which is wider than the
/// status-line meter.
const LEVEL_HISTORY: usize = 64;
const LIVE_TAIL_CHARS: usize = 56;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoicePhase {
    Recording,
    Transcribing,
}

pub(crate) struct VoiceInput {
    recording: NariRecording,
    waker_done: Arc<AtomicBool>,
    phase: VoicePhase,
    started: Instant,
    pressed_at: Instant,
    live: String,
    levels: VecDeque<f32>,
}

impl Drop for VoiceInput {
    fn drop(&mut self) {
        // Dropping an unfinished NariRecording cancels capture and the stream.
        self.waker_done.store(true, Ordering::SeqCst);
    }
}

/// Result of draining the active recording.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VoicePoll {
    /// Nothing to repaint.
    Idle,
    /// Meter, live text, or phase changed. Repaint.
    Changed,
    /// Final transcript, ready to send.
    Transcript(String),
}

impl App {
    pub(crate) fn voice_input_key_label(&self) -> Option<&str> {
        self.voice_input_key.label.as_deref()
    }

    fn voice_input_key_matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.voice_input_key
            .binding
            .as_ref()
            .is_some_and(|binding| binding.matches(code, modifiers))
    }

    /// Voice keys, handled before any other key routing so they work from
    /// every screen. Returns true when the event was consumed.
    pub(crate) fn handle_voice_key_event(&mut self, event: &KeyEvent) -> bool {
        if self.voice_input.is_some() && event.code == KeyCode::Esc {
            if event.kind == KeyEventKind::Press {
                let discarded = self
                    .voice_input
                    .as_ref()
                    .is_some_and(|voice| voice.phase == VoicePhase::Transcribing);
                self.cancel_voice_input(if discarded {
                    "Voice input discarded"
                } else {
                    "Voice input canceled"
                });
            }
            return true;
        }
        if !self.voice_input_key_matches(event.code, event.modifiers) {
            return false;
        }
        if event.kind != KeyEventKind::Press {
            // Repeats of a held key are part of the same press. Releases are
            // observed separately in `observe_voice_key_release`.
            return true;
        }
        let now = Instant::now();
        let previous = self.voice_input_last_press.replace(now);
        if previous.is_some_and(|at| now.duration_since(at) < REPEAT_DEBOUNCE) {
            return true;
        }
        self.toggle_voice_input();
        true
    }

    /// Hold-to-talk on terminals that report key releases: letting go of a
    /// held voice key sends. A quick tap keeps recording (toggle mode).
    pub(crate) fn observe_voice_key_release(&mut self, event: &KeyEvent) {
        if event.kind != KeyEventKind::Release {
            return;
        }
        let Some(binding) = self.voice_input_key.binding.as_ref() else {
            return;
        };
        // Modifiers may already be up when the key itself is released.
        if !event.code.eq(&binding.code) {
            return;
        }
        let held_long_enough = self.voice_input.as_ref().is_some_and(|voice| {
            voice.phase == VoicePhase::Recording && voice.pressed_at.elapsed() >= HOLD_TO_TALK
        });
        if held_long_enough {
            self.stop_voice_input();
        }
    }

    pub(crate) fn toggle_voice_input(&mut self) {
        match self.voice_input.as_ref().map(|voice| voice.phase) {
            None => self.start_voice_input(),
            Some(VoicePhase::Recording) => self.stop_voice_input(),
            Some(VoicePhase::Transcribing) => {
                self.set_status_notice("Still transcribing. Esc discards it.")
            }
        }
    }

    fn start_voice_input(&mut self) {
        let Some(key) = voice::nari_api_key() else {
            self.push_display_message(DisplayMessage::error(
                "Voice input needs a Nari API key. Set `NARI_API_KEY`, or add \
                 `NARI_API_KEY=...` to `~/.config/jcode/nari.env`."
                    .to_string(),
            ));
            self.set_status_notice("Voice input not configured");
            return;
        };
        let recorder = crate::config::config().dictation.recorder.clone();
        if !voice::NATIVE_CAPTURE
            && recorder.trim().is_empty()
            && voice::detect_recorders("").is_empty()
        {
            self.push_display_message(DisplayMessage::error(
                "Voice input found no microphone recorder. Install one of \
                 `pw-record` (PipeWire), `parecord` (PulseAudio), `arecord` \
                 (ALSA), `rec` (SoX), or `ffmpeg`, or set `[dictation] recorder` \
                 in `~/.jcode/config.toml`."
                    .to_string(),
            ));
            self.set_status_notice("No microphone recorder found");
            return;
        }
        voice::timing::begin();
        let cancel = Arc::new(AtomicBool::new(false));
        // Nonblocking: the microphone opens and the Nari handshake runs in the
        // background while audio buffers, so the first words are kept.
        let recording = match NariRecording::start_auto(cancel, &key, &recorder) {
            Ok(recording) => recording,
            Err(error) => {
                self.report_voice_error(&error);
                return;
            }
        };
        let waker_done = Arc::new(AtomicBool::new(false));
        spawn_voice_waker(recording.event_notify(), waker_done.clone());
        let now = Instant::now();
        self.note_client_interaction();
        self.voice_input = Some(VoiceInput {
            recording,
            waker_done,
            phase: VoicePhase::Recording,
            started: now,
            pressed_at: now,
            live: String::new(),
            levels: VecDeque::with_capacity(LEVEL_HISTORY),
        });
    }

    fn stop_voice_input(&mut self) {
        let Some(voice) = self.voice_input.as_mut() else {
            return;
        };
        if voice.phase != VoicePhase::Recording {
            return;
        }
        voice::timing::release();
        voice.recording.stop();
        voice.phase = VoicePhase::Transcribing;
    }

    pub(crate) fn cancel_voice_input(&mut self, notice: &str) {
        if self.voice_input.take().is_some() {
            self.set_status_notice(notice.to_string());
        }
    }

    fn report_voice_error(&mut self, error: &VoiceError) {
        let hint = match error {
            VoiceError::MicrophoneUnavailable if !voice::NATIVE_CAPTURE => {
                " Check that a microphone is connected, or set `[dictation] recorder`."
            }
            _ => "",
        };
        self.push_display_message(DisplayMessage::error(format!(
            "Voice input failed: {error}.{hint}"
        )));
        self.set_status_notice("Voice input failed");
    }

    /// Drain the active recording: meter level, live transcript, final result.
    pub(crate) fn poll_voice_input(&mut self) -> VoicePoll {
        let Some(voice) = self.voice_input.as_mut() else {
            return VoicePoll::Idle;
        };
        let mut changed = false;
        if voice.phase == VoicePhase::Recording {
            if voice.levels.len() == LEVEL_HISTORY {
                voice.levels.pop_front();
            }
            voice.levels.push_back(voice.recording.audio_level());
            changed = true;
        }
        let mut finished = drain_voice_events(voice, &mut changed);
        if finished.is_none() && voice.recording.is_finished() {
            // Re-check after observing worker completion so a final event
            // published just after the first drain is not lost.
            finished =
                drain_voice_events(voice, &mut changed).or(Some(Err(VoiceError::CaptureFailed)));
        }
        let Some(result) = finished else {
            return if changed {
                VoicePoll::Changed
            } else {
                VoicePoll::Idle
            };
        };
        self.voice_input = None;
        match result {
            Ok(text) if !text.trim().is_empty() => VoicePoll::Transcript(text),
            Ok(_) => {
                self.set_status_notice("No speech detected");
                VoicePoll::Changed
            }
            Err(VoiceError::Cancelled) => VoicePoll::Changed,
            Err(error) => {
                self.report_voice_error(&error);
                VoicePoll::Changed
            }
        }
    }

    /// Local (in-process) sessions: handle a voice wake from the bus.
    pub(crate) fn handle_voice_input_wake_local(&mut self) -> bool {
        match self.poll_voice_input() {
            VoicePoll::Idle => false,
            VoicePoll::Changed => true,
            VoicePoll::Transcript(text) => {
                super::remote::submit_voice_transcript(self, &text);
                true
            }
        }
    }

    /// Record push-to-talk state reported by `jcode companion`.
    pub(crate) fn apply_external_voice_activity(
        &mut self,
        activity: crate::protocol::VoiceActivity,
    ) {
        self.external_voice = match activity {
            crate::protocol::VoiceActivity::Idle => None,
            activity => Some((activity, Instant::now())),
        };
    }

    /// Companion push-to-talk status, unless stale or turned off.
    fn external_voice_status_line(&self) -> Option<(bool, String)> {
        if !crate::config::config().companion.voice_waveform {
            return None;
        }
        let (activity, updated) = self.external_voice.as_ref()?;
        external_voice_line(activity, updated.elapsed())
    }

    /// Status-line text while voice input is active: `(recording, text)`.
    /// Empty when the composer shows it instead.
    pub(crate) fn voice_input_status_line(&self) -> Option<(bool, String)> {
        if crate::config::config().dictation.composer_preview {
            return None;
        }
        self.voice_status_line_text()
    }

    /// Live voice state for the composer, built-in recording first, then
    /// companion push-to-talk.
    pub(crate) fn voice_composer_preview_state(&self) -> Option<crate::tui::VoiceComposerPreview> {
        if !crate::config::config().dictation.composer_preview {
            return None;
        }
        if let Some(voice) = self.voice_input.as_ref() {
            let key = self.voice_input_key_label().unwrap_or("/voice");
            let recording = voice.phase == VoicePhase::Recording;
            return Some(crate::tui::VoiceComposerPreview {
                recording,
                elapsed_secs: voice.started.elapsed().as_secs(),
                levels: voice.levels.iter().copied().collect(),
                live: voice.live.trim().to_string(),
                hint: if recording {
                    format!("{key} send · Esc cancel")
                } else {
                    "Esc discard".to_string()
                },
            });
        }
        if !crate::config::config().companion.voice_waveform {
            return None;
        }
        let (activity, updated) = self.external_voice.as_ref()?;
        external_voice_preview(activity, updated.elapsed())
    }

    fn voice_status_line_text(&self) -> Option<(bool, String)> {
        let Some(voice) = self.voice_input.as_ref() else {
            return self.external_voice_status_line();
        };
        let key = self.voice_input_key_label().unwrap_or("/voice");
        let live = live_tail(&voice.live);
        Some(match voice.phase {
            VoicePhase::Recording => {
                let secs = voice.started.elapsed().as_secs();
                let start = voice.levels.len().saturating_sub(METER_BARS);
                let meter: String = voice
                    .levels
                    .iter()
                    .skip(start)
                    .map(|level| meter_bar(*level))
                    .collect();
                let heard = if live.is_empty() {
                    "listening…".to_string()
                } else {
                    live
                };
                (
                    true,
                    format!(
                        "● {}:{:02} {meter} {heard} · {key} send · Esc cancel",
                        secs / 60,
                        secs % 60
                    ),
                )
            }
            VoicePhase::Transcribing => {
                let heard = if live.is_empty() {
                    String::new()
                } else {
                    format!(" {live}")
                };
                (false, format!("◌ Transcribing…{heard} · Esc discard"))
            }
        })
    }
}

/// Updates arrive every ~80 ms while recording. Past this, the sender is gone.
const EXTERNAL_VOICE_STALE: Duration = Duration::from_secs(3);
/// Transcribing can legitimately take a while with no updates.
const EXTERNAL_TRANSCRIBING_STALE: Duration = Duration::from_secs(60);

fn external_voice_preview(
    activity: &crate::protocol::VoiceActivity,
    age: Duration,
) -> Option<crate::tui::VoiceComposerPreview> {
    use crate::protocol::VoiceActivity;
    match activity {
        VoiceActivity::Recording {
            elapsed_ms,
            levels,
            live,
        } if age <= EXTERNAL_VOICE_STALE => Some(crate::tui::VoiceComposerPreview {
            recording: true,
            elapsed_secs: elapsed_ms / 1000,
            levels: levels.clone(),
            live: live.trim().to_string(),
            hint: "release to send".to_string(),
        }),
        VoiceActivity::Transcribing { live } if age <= EXTERNAL_TRANSCRIBING_STALE => {
            Some(crate::tui::VoiceComposerPreview {
                recording: false,
                elapsed_secs: 0,
                levels: Vec::new(),
                live: live.trim().to_string(),
                hint: String::new(),
            })
        }
        _ => None,
    }
}

fn external_voice_line(
    activity: &crate::protocol::VoiceActivity,
    age: Duration,
) -> Option<(bool, String)> {
    use crate::protocol::VoiceActivity;
    match activity {
        VoiceActivity::Recording {
            elapsed_ms,
            levels,
            live,
        } => {
            if age > EXTERNAL_VOICE_STALE {
                return None;
            }
            let secs = elapsed_ms / 1000;
            let start = levels.len().saturating_sub(METER_BARS);
            let meter: String = levels[start..].iter().map(|l| meter_bar(*l)).collect();
            let live = live_tail(live);
            let heard = if live.is_empty() {
                "listening…".to_string()
            } else {
                live
            };
            Some((
                true,
                format!(
                    "● {}:{:02} {meter} {heard} · release to send",
                    secs / 60,
                    secs % 60
                ),
            ))
        }
        VoiceActivity::Transcribing { live } => {
            if age > EXTERNAL_TRANSCRIBING_STALE {
                return None;
            }
            let live = live_tail(live);
            let heard = if live.is_empty() {
                String::new()
            } else {
                format!(" {live}")
            };
            Some((false, format!("◌ Transcribing…{heard}")))
        }
        VoiceActivity::Idle => None,
    }
}

fn drain_voice_events(
    voice: &mut VoiceInput,
    changed: &mut bool,
) -> Option<Result<String, VoiceError>> {
    // Bounded so a fast provider cannot monopolize the UI loop.
    for _ in 0..64 {
        match voice.recording.try_event()? {
            NariEvent::Started => {}
            NariEvent::Transcript(text) => {
                voice.live = text;
                *changed = true;
            }
            NariEvent::Finished(result) => return Some(result),
        }
    }
    None
}

/// Wakes the UI loop on every provider event and on a steady meter tick, so
/// the recording state repaints in every run loop without polling.
fn spawn_voice_waker(notify: Arc<tokio::sync::Notify>, done: Arc<AtomicBool>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        while !done.load(Ordering::SeqCst) {
            let _ = tokio::time::timeout(METER_TICK, notify.notified()).await;
            if done.load(Ordering::SeqCst) {
                break;
            }
            Bus::global().publish(BusEvent::VoiceInputWake);
        }
    });
}

/// RMS level to a meter glyph. Speech RMS sits around 0.02 to 0.2, so the
/// scale is compressed to keep normal speech visibly moving.
fn meter_bar(level: f32) -> char {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let scaled = (level.max(0.0) * 4.0).sqrt().min(1.0);
    BARS[(scaled * 7.0).round() as usize]
}

fn live_tail(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= LIVE_TAIL_CHARS {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - LIVE_TAIL_CHARS).collect();
    format!("…{}", tail.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_is_flat_for_silence_and_full_for_loud_speech() {
        assert_eq!(meter_bar(0.0), '▁');
        assert_eq!(meter_bar(-1.0), '▁');
        assert_eq!(meter_bar(0.25), '█');
        assert_eq!(meter_bar(1.0), '█');
        let speech = meter_bar(0.05);
        assert!(speech != '▁' && speech != '█', "{speech}");
    }

    #[test]
    fn external_voice_shows_meter_and_expires() {
        use crate::protocol::VoiceActivity;
        let recording = VoiceActivity::Recording {
            elapsed_ms: 65_000,
            levels: vec![0.0; 14].into_iter().chain([1.0]).collect(),
            live: "hello there".into(),
        };
        let (rec, line) = external_voice_line(&recording, Duration::ZERO).unwrap();
        assert!(rec);
        assert!(line.starts_with("● 1:05 "), "{line}");
        assert_eq!(
            line.chars().filter(|c| "▁▂▃▄▅▆▇█".contains(*c)).count(),
            METER_BARS
        );
        assert!(line.contains('█') && line.contains("hello there"));
        assert!(external_voice_line(&recording, Duration::from_secs(4)).is_none());
        let transcribing = VoiceActivity::Transcribing {
            live: String::new(),
        };
        assert!(external_voice_line(&transcribing, Duration::from_secs(4)).is_some());
        assert!(external_voice_line(&VoiceActivity::Idle, Duration::ZERO).is_none());
    }

    #[test]
    fn live_tail_keeps_the_most_recent_words() {
        assert_eq!(live_tail("  short  "), "short");
        let long = format!("{} the end", "word ".repeat(30));
        let tail = live_tail(&long);
        assert!(tail.starts_with('…'));
        assert!(tail.ends_with("the end"));
        assert!(tail.chars().count() <= LIVE_TAIL_CHARS + 1);
    }
}
