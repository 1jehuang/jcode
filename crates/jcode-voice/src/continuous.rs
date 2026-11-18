//! Explicitly opted-in continuous listening. Only locally segmented utterances reach Nari.
use crate::{NariSession, PcmRecording, VoiceError};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct ContinuousConfig {
    /// Require an explicit Jev address as well as request-shaped speech.
    pub require_jev: bool,
}
impl Default for ContinuousConfig {
    fn default() -> Self {
        Self { require_jev: true }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceState {
    Starting,
    Listening,
    Speaking,
    Transcribing,
    Suppressed,
}
#[derive(Clone)]
pub enum ContinuousEvent {
    State(VoiceState),
    Level(f32),
    Transcript(String),
    Rejected(String),
    Error(VoiceError),
    Stopped,
}
struct Events {
    queue: VecDeque<ContinuousEvent>,
    notify: Arc<tokio::sync::Notify>,
}
impl Events {
    fn push(&mut self, event: ContinuousEvent) {
        // Coalesce visual telemetry. Bound even when the consumer stops polling.
        match &event {
            ContinuousEvent::Level(_) => self
                .queue
                .retain(|e| !matches!(e, ContinuousEvent::Level(_))),
            ContinuousEvent::State(_) => self
                .queue
                .retain(|e| !matches!(e, ContinuousEvent::State(_))),
            _ => {}
        }
        if self.queue.len() >= 32 {
            self.queue.pop_front();
        }
        self.queue.push_back(event);
        self.notify.notify_one();
    }
}
fn emit(events: &Mutex<Events>, event: ContinuousEvent) {
    events.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}
/// Nonblocking owner. Drop cancels capture and any in-flight transcription.
/// The microphone is opened only by this explicit constructor, never by capability checks.
pub struct ContinuousRecording {
    cancel: Arc<AtomicBool>,
    suppressed: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    level: Arc<AtomicU32>,
    events: Arc<Mutex<Events>>,
    worker: std::thread::JoinHandle<()>,
}
impl ContinuousRecording {
    pub fn start_auto(
        cancel: Arc<AtomicBool>,
        key: &str,
        recorder: &str,
        config: ContinuousConfig,
    ) -> Result<Self, VoiceError> {
        if cancel.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        if key.trim().is_empty() {
            return Err(VoiceError::NariNotConfigured);
        }
        let suppressed = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(0));
        let level = Arc::new(AtomicU32::new(0));
        let events = Arc::new(Mutex::new(Events {
            queue: VecDeque::new(),
            notify: Arc::new(tokio::sync::Notify::new()),
        }));
        let (c, s, g, l, e) = (
            cancel.clone(),
            suppressed.clone(),
            epoch.clone(),
            level.clone(),
            events.clone(),
        );
        let (key, recorder) = (key.to_owned(), recorder.to_owned());
        let worker = std::thread::Builder::new()
            .name("voice-continuous".into())
            .spawn(move || {
                let result = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => {
                        let result = rt.block_on(run(
                            c.clone(),
                            s,
                            g,
                            l.clone(),
                            e.clone(),
                            key,
                            recorder,
                            config,
                        ));
                        rt.shutdown_background();
                        result
                    }
                    Err(_) => Err(VoiceError::CaptureFailed),
                };
                l.store(0, Ordering::Relaxed);
                if let Err(error) = result {
                    if error != VoiceError::Cancelled {
                        emit(&e, ContinuousEvent::Error(error));
                    }
                }
                emit(&e, ContinuousEvent::Stopped);
            })
            .map_err(|_| VoiceError::CaptureFailed)?;
        Ok(Self {
            cancel,
            suppressed,
            epoch,
            level,
            events,
            worker,
        })
    }
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    /// Invalidates captured and pending speech on both edges. TTS cannot turn into a request.
    pub fn set_suppressed(&self, value: bool) {
        if self.suppressed.swap(value, Ordering::SeqCst) != value {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        if value {
            self.level.store(0, Ordering::Relaxed);
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .retain(|event| {
                    !matches!(
                        event,
                        ContinuousEvent::Transcript(_) | ContinuousEvent::Rejected(_)
                    )
                });
        }
    }
    pub fn audio_level(&self) -> f32 {
        if self.cancel.load(Ordering::SeqCst)
            || self.suppressed.load(Ordering::SeqCst)
            || self.is_finished()
        {
            0.0
        } else {
            f32::from_bits(self.level.load(Ordering::Relaxed))
        }
    }
    pub fn try_event(&self) -> Option<ContinuousEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .pop_front()
    }
    pub fn event_notify(&self) -> Arc<tokio::sync::Notify> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .notify
            .clone()
    }
    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }
}
impl Drop for ContinuousRecording {
    fn drop(&mut self) {
        self.stop();
    }
}

// 20 ms frames, 300 ms pre-roll, 180 ms confirmed voice, 700 ms end silence,
// 20 seconds maximum utterance. All sample buffers have hard upper bounds.
#[derive(Default)]
struct Segmenter {
    pending: VecDeque<i16>,
    pre: VecDeque<i16>,
    speech: Vec<i16>,
    voiced: usize,
    quiet: usize,
    active: bool,
}
impl Segmenter {
    fn push(&mut self, input: &[i16]) -> Vec<Vec<i16>> {
        let mut output = Vec::new();
        for &sample in input {
            self.pending.push_back(sample);
            if self.pending.len() < 320 {
                continue;
            }
            let frame: Vec<_> = self.pending.drain(..).collect();
            let voice = rms(&frame) >= 0.018;
            if !self.active {
                self.pre.extend(frame.iter().copied());
                while self.pre.len() > 4800 {
                    self.pre.pop_front();
                }
                self.voiced = if voice { self.voiced + 320 } else { 0 };
                if self.voiced >= 2880 {
                    self.active = true;
                    self.speech.extend(self.pre.drain(..));
                    self.quiet = 0;
                }
            } else {
                self.speech.extend(frame);
                self.quiet = if voice { 0 } else { self.quiet + 320 };
                if self.quiet >= 11200 || self.speech.len() >= 320000 {
                    output.push(std::mem::take(&mut self.speech));
                    self.active = false;
                    self.voiced = 0;
                    self.quiet = 0;
                }
            }
        }
        output
    }
}
fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples
        .iter()
        .map(|s| (f64::from(*s) / 32768.0).powi(2))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt() as f32
}
/// Conservative local prefilter, not an authorization boundary. Rejects casual
/// acknowledgements, name-only addresses, quoted/reported addresses and speech
/// with no recognizable request shape. Callers may apply a stricter classifier.
pub fn directed_transcript(text: &str, require_jev: bool) -> Option<String> {
    let words: Vec<_> = text.split_whitespace().collect();
    let normalized: Vec<_> = words
        .iter()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    let mut start = 0;
    if matches!(
        normalized.first().map(String::as_str),
        Some("hey" | "okay" | "ok" | "hi")
    ) {
        start += 1;
    }
    if normalized.get(start).is_some_and(|w| w == "jev") {
        start += 1;
    } else if require_jev {
        return None;
    }
    let request = normalized.get(start)?.as_str();
    if !matches!(
        request,
        "please"
            | "can"
            | "could"
            | "would"
            | "will"
            | "what"
            | "why"
            | "how"
            | "where"
            | "when"
            | "who"
            | "show"
            | "open"
            | "close"
            | "help"
            | "explain"
            | "tell"
            | "find"
            | "search"
            | "look"
            | "check"
            | "read"
            | "write"
            | "create"
            | "make"
            | "fix"
            | "change"
            | "update"
            | "run"
            | "stop"
            | "cancel"
            | "summarize"
            | "review"
            | "implement"
            | "add"
            | "remove"
            | "delete"
            | "switch"
            | "go"
            | "send"
            | "start"
    ) {
        return None;
    }
    if normalized.len() <= start + 1 && !matches!(request, "stop" | "cancel") {
        return None;
    }
    Some(words[start..].join(" "))
}

async fn open_capture(
    cancel: Arc<AtomicBool>,
    recorder: String,
) -> Result<(PcmRecording, tokio::sync::mpsc::Receiver<Vec<i16>>), VoiceError> {
    tokio::task::spawn_blocking(move || {
        #[cfg(feature = "voice-capture")]
        if recorder.trim().is_empty() {
            return crate::MicrophoneRecording::start_pcm_cancellable(cancel);
        }
        crate::command_capture::start_pcm(cancel, crate::detect_recorders(&recorder))
    })
    .await
    .map_err(|_| VoiceError::CaptureFailed)?
}
async fn transcribe(
    key: String,
    samples: Vec<i16>,
    cancel: Arc<AtomicBool>,
) -> Result<String, VoiceError> {
    let session = NariSession::connect(&key, cancel.clone()).await?;
    let (tx, rx) = crate::nari_pcm_channel();
    let send = async move {
        for chunk in samples.chunks(crate::NARI_PCM_CHUNK_SAMPLES) {
            if tx.send(chunk.to_vec()).await.is_err() {
                break;
            }
        }
    };
    let (_, result) = tokio::join!(send, session.run(rx, |_| {}));
    result
}
async fn run(
    cancel: Arc<AtomicBool>,
    suppressed: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    level: Arc<AtomicU32>,
    events: Arc<Mutex<Events>>,
    key: String,
    recorder: String,
    config: ContinuousConfig,
) -> Result<(), VoiceError> {
    emit(&events, ContinuousEvent::State(VoiceState::Starting));
    let mut segmenter = Segmenter::default();
    let mut generation = epoch.load(Ordering::SeqCst);
    let mut resume_at = Instant::now();
    let mut pending = VecDeque::new();
    let mut job: Option<(
        tokio::task::JoinHandle<Result<String, VoiceError>>,
        Arc<AtomicBool>,
        u64,
    )> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    // Each device lease rotates before the push-to-talk five-minute safety cap.
    loop {
        let setup_cancel = Arc::new(AtomicBool::new(false));
        let setup = open_capture(setup_cancel.clone(), recorder.clone());
        let (mic, mut pcm) = tokio::select! {
            biased;
            _ = crate::nari::cancelled(&cancel) => { setup_cancel.store(true,Ordering::SeqCst); return Err(VoiceError::Cancelled); },
            result = setup => result?,
        };
        let rotate = tokio::time::sleep(Duration::from_secs(240));
        tokio::pin!(rotate);
        let mut last_audio = Instant::now();
        emit(&events, ContinuousEvent::State(VoiceState::Listening));
        let result = loop {
            tokio::select! {
                biased;
                _ = crate::nari::cancelled(&cancel) => break Err(VoiceError::Cancelled),
                _ = &mut rotate => break Ok(()),
                chunk = pcm.recv() => {
                    let Some(chunk) = chunk else { break Err(VoiceError::CaptureFailed); };
                    last_audio = Instant::now();
                    let next = epoch.load(Ordering::SeqCst);
                    if next != generation {
                        generation = next; segmenter = Segmenter::default(); pending.clear();
                        if let Some((handle,token,_)) = job.take() { token.store(true,Ordering::SeqCst); handle.abort(); }
                        // Drain stale capture buffers and allow loudspeaker decay after TTS.
                        resume_at = Instant::now() + Duration::from_millis(500);
                    }
                    if suppressed.load(Ordering::SeqCst) || Instant::now() < resume_at {
                        level.store(0,Ordering::Relaxed); continue;
                    }
                    level.store(rms(&chunk).to_bits(),Ordering::Relaxed);
                    for utterance in segmenter.push(&chunk) {
                        if pending.len() < 2 { pending.push_back(utterance); }
                        // Drop excess utterances, never build an unbounded upload backlog.
                    }
                },
                _ = tick.tick() => {
                    if Instant::now().duration_since(last_audio) > Duration::from_secs(5) { break Err(VoiceError::CaptureFailed); }
                    if job.as_ref().is_some_and(|(h,_,_)|h.is_finished()) {
                        let (handle,_,job_generation) = job.take().unwrap();
                        let result = handle.await.map_err(|_|VoiceError::CaptureFailed)?;
                        if job_generation == epoch.load(Ordering::SeqCst) && !suppressed.load(Ordering::SeqCst) {
                            match result {
                                Ok(text) => {
                                    if directed_transcript(&text,config.require_jev).is_some() {
                                        let mut output = events.lock().unwrap_or_else(|e|e.into_inner());
                                        if job_generation == epoch.load(Ordering::SeqCst) && !suppressed.load(Ordering::SeqCst) {
                                            output.push(ContinuousEvent::Transcript(text));
                                        }
                                    }
                                    else if !text.trim().is_empty() { emit(&events,ContinuousEvent::Rejected(text)); }
                                },
                                Err(error) => break Err(error), // No automatic network/auth retry loop.
                            }
                        }
                    }
                    if job.is_none() && !suppressed.load(Ordering::SeqCst) && generation == epoch.load(Ordering::SeqCst) {
                        if let Some(samples) = pending.pop_front() {
                            let token = Arc::new(AtomicBool::new(false));
                            job = Some((tokio::spawn(transcribe(key.clone(),samples,token.clone())),token,generation));
                        }
                    }
                    let state = if suppressed.load(Ordering::SeqCst) { VoiceState::Suppressed }
                        else if segmenter.active { VoiceState::Speaking }
                        else if job.is_some() { VoiceState::Transcribing } else { VoiceState::Listening };
                    emit(&events,ContinuousEvent::State(state));
                    emit(&events,ContinuousEvent::Level(if state == VoiceState::Suppressed { 0.0 } else { f32::from_bits(level.load(Ordering::Relaxed)) }));
                },
            }
        };
        mic.stop();
        let capture = tokio::task::spawn_blocking(move || mic.finish())
            .await
            .map_err(|_| VoiceError::CaptureFailed)?;
        if result.is_err() || capture.is_err() {
            if let Some((handle, token, _)) = job.take() {
                token.store(true, Ordering::SeqCst);
                handle.abort();
            }
            result?;
            capture?;
        }
        // Do not join partial speech across a device restart.
        segmenter = Segmenter::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_never_segments() {
        let mut s = Segmenter::default();
        for _ in 0..1000 {
            assert!(s.push(&[0; 1600]).is_empty());
        }
        assert!(s.pre.len() <= 4800);
    }
    #[test]
    fn speech_has_preroll_and_trailing_silence() {
        let mut s = Segmenter::default();
        s.push(&[0; 4800]);
        assert!(s.push(&[3000; 16000]).is_empty());
        let out = s.push(&[0; 11200]);
        assert_eq!(out.len(), 1);
        assert!(out[0].len() <= 320000);
        assert!(!s.active);
    }
    #[test]
    fn brief_noise_is_not_speech() {
        let mut s = Segmenter::default();
        s.push(&[10000; 1600]);
        assert!(s.push(&[0; 16000]).is_empty());
    }
    #[test]
    fn maximum_utterance_is_bounded() {
        let mut s = Segmenter::default();
        let out = s.push(&vec![3000; 400000]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 320000);
    }
    #[test]
    fn conservative_gate() {
        assert_eq!(
            directed_transcript("Hey Jev, open settings", true).as_deref(),
            Some("open settings")
        );
        for text in [
            "open settings",
            "thanks Jev",
            "Jev",
            "Jev is great",
            "I told Jev to open settings",
            "Jev, okay",
            "okay Jev",
        ] {
            assert!(directed_transcript(text, true).is_none(), "{text}");
        }
        assert!(directed_transcript("open settings", false).is_some());
        assert!(directed_transcript("that's interesting", false).is_none());
    }
    #[test]
    fn bounded_events() {
        let mut e = Events {
            queue: VecDeque::new(),
            notify: Arc::new(tokio::sync::Notify::new()),
        };
        for _ in 0..1000 {
            e.push(ContinuousEvent::Rejected("no".into()));
            e.push(ContinuousEvent::Level(0.1));
        }
        assert!(e.queue.len() <= 32);
        assert_eq!(
            e.queue
                .iter()
                .filter(|e| matches!(e, ContinuousEvent::Level(_)))
                .count(),
            1
        );
    }
}
