//! Dedicated-thread native streaming capture and nonblocking desktop handle.
use super::{resample::Resampler, *};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
};

/// Native mono 16k PCM capture. Stop is a signal, EOF follows buffered chunks.
/// Dropping an unfinished handle cancels, never blocks the UI on native teardown.
pub struct PcmRecording {
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<(), VoiceError>>>,
}
impl MicrophoneRecording {
    /// Blocking setup, call off the UI thread and only after explicit user consent.
    pub fn start_pcm_cancellable(
        cancel: Arc<AtomicBool>,
    ) -> Result<(PcmRecording, tokio::sync::mpsc::Receiver<Vec<i16>>), VoiceError> {
        PcmRecording::start(cancel)
    }
}
impl PcmRecording {
    fn start(
        cancel: Arc<AtomicBool>,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<Vec<i16>>), VoiceError> {
        if cancel.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        let (tx, rx) = super::nari::capture_pcm_channel();
        let (ready, started) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let worker_level = level.clone();
        let (c, s) = (cancel.clone(), stop.clone());
        let worker = thread::Builder::new()
            .name("voice-pcm".into())
            .spawn(move || {
                let result = capture(tx, &ready, c, s, worker_level);
                if let Err(e) = &result {
                    let _ = ready.try_send(Err(e.clone()));
                }
                result
            })
            .map_err(|_| VoiceError::CaptureFailed)?;
        let started = wait_started(&started, &cancel);
        super::timing::mark("pcm start returned");
        match started {
            Ok(Ok(())) => Ok((
                Self {
                    level,
                    stop,
                    cancel,
                    worker: Some(worker),
                },
                rx,
            )),
            Ok(Err(e)) => Err(e),
            _ => Err(VoiceError::CaptureFailed),
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    pub fn is_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(|w| w.is_finished())
    }
    /// Join off the UI thread to observe device/backpressure failures after EOF.
    pub fn finish(mut self) -> Result<(), VoiceError> {
        self.stop();
        self.worker
            .take()
            .ok_or(VoiceError::CaptureFailed)?
            .join()
            .map_err(|_| VoiceError::CaptureFailed)?
    }
}
impl Drop for PcmRecording {
    fn drop(&mut self) {
        if self.worker.as_ref().is_some_and(|w| !w.is_finished()) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        self.stop();
    }
}
fn wait_started(
    ready: &mpsc::Receiver<Result<(), VoiceError>>,
    cancel: &AtomicBool,
) -> Result<Result<(), VoiceError>, mpsc::RecvError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Err(VoiceError::Cancelled));
        }
        if std::time::Instant::now() >= deadline {
            cancel.store(true, Ordering::SeqCst);
            return Ok(Err(VoiceError::Timeout));
        }
        match ready.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return Ok(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(mpsc::RecvError),
        }
    }
}

struct Chunker {
    level: Arc<AtomicU32>,
    resampler: Resampler,
    chunk: Vec<i16>,
    tx: tokio::sync::mpsc::Sender<Vec<i16>>,
    failed: bool,
    samples: usize,
    first_callback: bool,
    first_voice: bool,
}
impl Chunker {
    fn push<T>(&mut self, data: &[T], channels: usize)
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        if self.failed {
            return;
        }
        if !self.first_callback {
            self.first_callback = true;
            super::timing::mark("first microphone callback");
        }
        let mut energy = 0.0f64;
        let mut frames = 0usize;
        for frame in data.chunks_exact(channels) {
            if self.samples >= 16000 * MAX_RECORDING_DURATION.as_secs() as usize {
                break;
            }
            let mono = frame
                .iter()
                .map(|s| {
                    let sample = <f32 as cpal::FromSample<T>>::from_sample_(*s);
                    if sample.is_finite() {
                        sample.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .sum::<f32>()
                / channels as f32;
            energy += f64::from(mono).powi(2);
            frames += 1;
            let before = self.chunk.len();
            self.resampler.push(mono, &mut self.chunk);
            // Upsampling can emit two output samples for one native frame.
            // Clamp this final frame as well as the filter tail.
            let remaining = 16000 * MAX_RECORDING_DURATION.as_secs() as usize - self.samples;
            self.chunk.truncate(before + remaining);
            self.samples += self.chunk.len() - before;
            if self.chunk.len() >= 1600 {
                self.flush();
                if self.failed {
                    break;
                }
            }
        }
        let rms = if frames == 0 {
            0.0
        } else {
            (energy / frames as f64).sqrt() as f32
        };
        if !self.first_voice && rms > 0.02 {
            self.first_voice = true;
            super::timing::mark("first audible input (rms > 0.02)");
        }
        self.level
            .store(rms.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    fn finish(&mut self, limit: usize) {
        let remaining = limit.saturating_sub(self.samples);
        let before = self.chunk.len();
        self.resampler.finish(&mut self.chunk);
        self.chunk.truncate(before + remaining);
        self.flush();
    }
    fn flush(&mut self) {
        if self.chunk.is_empty() {
            return;
        }
        let chunk = std::mem::replace(&mut self.chunk, Vec::with_capacity(1602));
        // Never block an audio callback. Fail closed rather than silently lose audio.
        if self.tx.try_send(chunk).is_err() {
            self.failed = true;
        }
    }
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<Chunker>>,
) -> Result<cpal::Stream, VoiceError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let errors = state.clone();
    let channels = config.channels as usize;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if let Ok(mut state) = state.lock() {
                    state.push(data, channels);
                }
            },
            move |_| {
                if let Ok(mut state) = errors.lock() {
                    state.failed = true;
                }
            },
            None,
        )
        .map_err(|_| VoiceError::MicrophoneUnavailable)
}
fn capture(
    tx: tokio::sync::mpsc::Sender<Vec<i16>>,
    ready: &mpsc::SyncSender<Result<(), VoiceError>>,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
) -> Result<(), VoiceError> {
    // Declared before the stream so teardown completes before the final reset.
    struct ResetLevel(Arc<AtomicU32>);
    impl Drop for ResetLevel {
        fn drop(&mut self) {
            self.0.store(0, Ordering::Relaxed);
        }
    }
    let _reset_level = ResetLevel(level.clone());
    if cancel.load(Ordering::SeqCst) {
        return Err(VoiceError::Cancelled);
    }
    super::timing::mark("capture thread started");
    let device = cpal::default_host()
        .default_input_device()
        .ok_or(VoiceError::MicrophoneUnavailable)?;
    let supported = device
        .default_input_config()
        .map_err(|_| VoiceError::MicrophoneUnavailable)?;
    super::timing::mark("microphone device opened");
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    if config.channels == 0 {
        return Err(VoiceError::MicrophoneUnavailable);
    }
    let state = Arc::new(Mutex::new(Chunker {
        level,
        resampler: Resampler::new(config.sample_rate.0)?,
        chunk: Vec::with_capacity(1602),
        tx,
        failed: false,
        samples: 0,
        first_callback: false,
        first_voice: false,
    }));
    macro_rules! build {
        ($t:ty) => {
            build::<$t>(&device, &config, state.clone())?
        };
    }
    let stream = match format {
        cpal::SampleFormat::I8 => build!(i8),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::I64 => build!(i64),
        cpal::SampleFormat::U8 => build!(u8),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::U32 => build!(u32),
        cpal::SampleFormat::U64 => build!(u64),
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::F64 => build!(f64),
        _ => return Err(VoiceError::MicrophoneUnavailable),
    };
    if cancel.load(Ordering::SeqCst) {
        return Err(VoiceError::Cancelled);
    }
    super::timing::mark("input stream built");
    stream
        .play()
        .map_err(|_| VoiceError::MicrophoneUnavailable)?;
    super::timing::mark("input stream playing");
    // PipeWire can take hundreds of ms to deliver the first buffer after a cold
    // start. Report ready only once audio flows, so the recording indicator never
    // precedes capture. Bounded so a quiet device still starts.
    let first_audio_deadline = std::time::Instant::now() + Duration::from_millis(1500);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        let state = state.lock().map_err(|_| VoiceError::CaptureFailed)?;
        if state.failed {
            return Err(VoiceError::CaptureFailed);
        }
        if state.first_callback || std::time::Instant::now() >= first_audio_deadline {
            break;
        }
        drop(state);
        thread::sleep(Duration::from_millis(2));
    }
    super::timing::mark("capture signalled ready");
    let _ = ready.send(Ok(()));
    let deadline = std::time::Instant::now() + MAX_RECORDING_DURATION;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        if stop.load(Ordering::SeqCst) || std::time::Instant::now() >= deadline {
            break;
        }
        {
            let state = state.lock().map_err(|_| VoiceError::CaptureFailed)?;
            if state.failed {
                return Err(VoiceError::CaptureFailed);
            }
            if state.samples >= 16000 * MAX_RECORDING_DURATION.as_secs() as usize {
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    let mut state = state.lock().map_err(|_| VoiceError::CaptureFailed)?;
    state.finish(16000 * MAX_RECORDING_DURATION.as_secs() as usize);
    if state.failed {
        Err(VoiceError::CaptureFailed)
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct Events {
    queue: VecDeque<NariEvent>,
    finished: bool,
}
impl Events {
    fn push(&mut self, event: NariEvent) {
        if matches!(event, NariEvent::Finished(_)) {
            self.finished = true;
        }
        // A slow UI needs only the latest full revision. Started and Finished survive.
        if matches!(event, NariEvent::Transcript(_)) {
            self.queue
                .retain(|e| !matches!(e, NariEvent::Transcript(_)));
        }
        self.queue.push_back(event);
    }
}
/// Unified network + microphone operation. Construction, every handle method
/// and Drop are nonblocking, so a push-to-talk UI can show recording at the
/// press. The microphone opens and the provider handshake runs concurrently in
/// the background. PCM is buffered until session.configured, so speech during
/// setup is kept. Setup failures arrive as `NariEvent::Finished(Err(_))`.
pub struct NariRecording {
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    events: Arc<Mutex<Events>>,
    worker: thread::JoinHandle<()>,
    // Set once the native stream exists. Reads zero until then.
    level: Arc<std::sync::OnceLock<Arc<AtomicU32>>>,
}
impl NariRecording {
    pub fn start_cancellable(cancel: Arc<AtomicBool>, key: &str) -> Result<Self, VoiceError> {
        Self::start_with(
            cancel,
            key,
            super::nari::URL,
            MicrophoneRecording::start_pcm_cancellable,
        )
    }
    fn start_with(
        cancel: Arc<AtomicBool>,
        key: &str,
        url: &str,
        factory: impl FnOnce(
            Arc<AtomicBool>,
        ) -> Result<
            (PcmRecording, tokio::sync::mpsc::Receiver<Vec<i16>>),
            VoiceError,
        > + Send
        + 'static,
    ) -> Result<Self, VoiceError> {
        if cancel.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        let url = url.to_owned();
        let key = key.to_owned();
        let events = Arc::new(Mutex::new(Events::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(std::sync::OnceLock::new());
        let (c, s, e, l) = (cancel.clone(), stop.clone(), events.clone(), level.clone());
        let worker = thread::Builder::new()
            .name("voice-nari".into())
            .spawn(move || {
                let result = (|| {
                    // Two workers so the provider handshake (native root loading
                    // and TLS setup are CPU-bound) never delays mic readiness.
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .thread_name("voice-nari-rt")
                        .enable_all()
                        .build()
                        .map_err(|_| VoiceError::CaptureFailed)?;
                    let result = runtime.block_on(async {
                        super::timing::mark("nari worker started");
                        // The press is the consent to record. Open the microphone
                        // while the provider handshake is in flight and buffer PCM
                        // until it is configured, so the first words are never lost.
                        let connect = tokio::spawn({
                            let c = c.clone();
                            async move { NariSession::connect_to(&url, &key, c).await }
                        });
                        let setup_cancel = Arc::new(AtomicBool::new(false));
                        let factory_cancel = setup_cancel.clone();
                        let (mic, pcm) = tokio::select! {
                            biased;
                            _ = super::nari::cancelled(&c) => { setup_cancel.store(true, Ordering::SeqCst); return Err(VoiceError::Cancelled); },
                            result = tokio::task::spawn_blocking(move || factory(factory_cancel)) => result.map_err(|_|VoiceError::CaptureFailed)??,
                        };
                        let _ = l.set(mic.level.clone());
                        super::timing::mark("microphone capturing");
                        // Forward release even while still connecting. Buffered
                        // audio then drains and commits once the session is up.
                        let mic_stop = mic.stop.clone();
                        let stop_signal = s.clone();
                        tokio::spawn(async move {
                            loop {
                                if stop_signal.load(Ordering::SeqCst) {
                                    mic_stop.store(true, Ordering::SeqCst);
                                    return;
                                }
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        });
                        // Dropping `mic` on failure cancels the unfinished capture.
                        let session = connect.await.map_err(|_| VoiceError::CaptureFailed)??;
                        let stream = session.run(pcm, |event| {
                            if !matches!(event, NariEvent::Finished(_))
                                && let Ok(mut events) = e.lock() {
                                    events.push(event);
                                }
                        });
                        let result = stream.await;
                        mic.stop();
                        // Native stream is owned by its capture thread, never the UI.
                        let capture = mic.finish();
                        match (result, capture) {
                            (Err(err), _) => Err(err),
                            (Ok(_), Err(err)) => Err(err),
                            (Ok(text), Ok(())) => Ok(text),
                        }
                    });
                    runtime.shutdown_background();
                    result
                })();
                if let Ok(mut events) = e.lock() {
                    events.push(NariEvent::Finished(result));
                }
            })
            .map_err(|_| VoiceError::CaptureFailed)?;
        Ok(Self {
            cancel,
            stop,
            events,
            worker,
            level,
        })
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    /// Latest microphone callback's linear RMS of the downmixed mono signal,
    /// before resampling, normalized to `0.0..=1.0` (not decibels).
    /// Silence is zero. No smoothing or artificial animation is applied.
    /// Returns zero after stop, cancellation, or completion. Polling is lock-free
    /// and does not consume events or audio. Requires the `voice-capture` feature.
    pub fn audio_level(&self) -> f32 {
        if self.stop.load(Ordering::SeqCst)
            || self.cancel.load(Ordering::SeqCst)
            || self.is_finished()
        {
            return 0.0;
        }
        self.level
            .get()
            .map_or(0.0, |level| f32::from_bits(level.load(Ordering::Relaxed)))
    }
    pub fn try_event(&self) -> Option<NariEvent> {
        self.events.lock().ok()?.queue.pop_front()
    }
    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }
}
impl Drop for NariRecording {
    fn drop(&mut self) {
        if !self.events.lock().is_ok_and(|events| events.finished) && !self.worker.is_finished() {
            self.cancel.store(true, Ordering::SeqCst);
        }
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn level_chunker() -> Chunker {
        let (tx, _rx) = nari_pcm_channel();
        Chunker {
            level: Arc::new(AtomicU32::new(0)),
            resampler: Resampler::new(48000).unwrap(),
            chunk: Vec::new(),
            tx,
            failed: false,
            samples: 0,
            first_callback: false,
            first_voice: false,
        }
    }
    fn level(c: &Chunker) -> f32 {
        f32::from_bits(c.level.load(Ordering::Relaxed))
    }
    #[test]
    fn callback_level_is_latest_rms_not_peak_or_signed_average() {
        let mut c = level_chunker();
        assert_eq!(level(&c), 0.0);
        c.push(&[0.5f32, -0.5, 0.5, -0.5], 1);
        assert_eq!(level(&c), 0.5);
        c.push(&[1.0f32, 0.0, -1.0, 0.0], 1);
        assert!((level(&c) - 0.5f32.sqrt()).abs() < 1e-6);
        c.push(&[0.0f32; 4], 1);
        assert_eq!(level(&c), 0.0);
        c.push::<f32>(&[], 1);
        assert_eq!(level(&c), 0.0);
    }
    #[test]
    fn callback_level_downmixes_and_normalizes_native_formats() {
        let mut c = level_chunker();
        c.push(&[0.75f32, 0.25, -0.75, -0.25], 2);
        assert_eq!(level(&c), 0.5);
        c.push(&[1.0f32, -1.0], 2);
        assert_eq!(level(&c), 0.0);
        c.push(&[16384i16, -16384], 1);
        assert_eq!(level(&c), 0.5);
        c.push(&[32768u16; 2], 1);
        assert_eq!(level(&c), 0.0);
        c.push(&[49152u16, 16384], 1);
        assert_eq!(level(&c), 0.5);
        c.push(&[0.25f64, -0.25], 1);
        assert_eq!(level(&c), 0.25);
    }
    #[test]
    fn callback_level_is_finite_and_bounded_for_invalid_float_samples() {
        let mut c = level_chunker();
        c.push(&[2.0f32, -2.0], 1);
        assert_eq!(level(&c), 1.0);
        c.push(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY], 1);
        assert_eq!(level(&c), 0.0);
    }
    #[test]
    fn recording_level_reads_callback_atomic_without_consuming_events() {
        let mut c = level_chunker();
        let (release, wait) = mpsc::channel();
        let recording = NariRecording {
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            level: Arc::new(std::sync::OnceLock::from(c.level.clone())),
            events: Arc::new(Mutex::new(Events::default())),
            worker: thread::spawn(move || {
                wait.recv().unwrap();
            }),
        };
        recording.events.lock().unwrap().push(NariEvent::Started);
        thread::spawn(move || c.push(&[0.5f32; 100], 1))
            .join()
            .unwrap();
        assert_eq!(recording.audio_level(), 0.5);
        assert_eq!(recording.audio_level(), 0.5);
        assert!(matches!(recording.try_event(), Some(NariEvent::Started)));
        recording.cancel.store(true, Ordering::SeqCst);
        assert_eq!(recording.audio_level(), 0.0);
        recording.cancel.store(false, Ordering::SeqCst);
        recording.stop();
        assert_eq!(recording.audio_level(), 0.0);
        recording.stop.store(false, Ordering::SeqCst);
        release.send(()).unwrap();
        while !recording.is_finished() {
            thread::yield_now();
        }
        assert_eq!(recording.audio_level(), 0.0);
    }
    #[test]
    fn cancelled_constructors_never_open_microphone() {
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            MicrophoneRecording::start_pcm_cancellable(cancel.clone()),
            Err(VoiceError::Cancelled)
        ));
        assert!(matches!(
            NariRecording::start_cancellable(cancel, "test"),
            Err(VoiceError::Cancelled)
        ));
    }
    #[test]
    fn bounded_chunks_downmix_and_fail_on_backpressure() {
        let (tx, mut rx) = nari_pcm_channel();
        let mut c = Chunker {
            level: Arc::new(AtomicU32::new(0)),
            resampler: Resampler::new(48000).unwrap(),
            chunk: Vec::new(),
            tx,
            failed: false,
            samples: 0,
            first_callback: false,
            first_voice: false,
        };
        c.push(&vec![0.5f32; 48000 * 2], 2);
        let mut count = 0;
        while let Ok(chunk) = rx.try_recv() {
            assert!(chunk.len() <= 1601);
            assert!(chunk.iter().skip(50).all(|s| (*s - 16384).abs() < 2));
            count += chunk.len();
        }
        assert!(count > 14000);
        assert!(!c.failed);
        c.push(&vec![0.0f32; 48000 * 6], 2);
        assert!(c.failed);
    }
    #[test]
    fn events_coalesce_but_preserve_lifecycle() {
        let mut e = Events::default();
        e.push(NariEvent::Started);
        for _ in 0..1000 {
            e.push(NariEvent::Transcript("latest".into()));
        }
        e.push(NariEvent::Finished(Ok("latest".into())));
        assert_eq!(e.queue.len(), 3);
        assert!(matches!(e.queue.front(), Some(NariEvent::Started)));
    }
    #[test]
    fn stop_preserves_token_drop_cancels_without_joining() {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let s = stop.clone();
        let worker = thread::spawn(move || {
            while !c.load(Ordering::SeqCst) && !s.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });
        let recording = PcmRecording {
            level: Arc::new(AtomicU32::new(0)),
            cancel: cancel.clone(),
            stop,
            worker: Some(worker),
        };
        recording.stop();
        recording.finish().unwrap();
        assert!(!cancel.load(Ordering::SeqCst));
        let c = cancel.clone();
        let worker = thread::spawn(move || {
            while !c.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });
        drop(PcmRecording {
            level: Arc::new(AtomicU32::new(0)),
            cancel: cancel.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Some(worker),
        });
        assert!(cancel.load(Ordering::SeqCst));
    }
    fn fake_capture(
        cancel: Arc<AtomicBool>,
        released: Arc<AtomicBool>,
    ) -> (PcmRecording, tokio::sync::mpsc::Receiver<Vec<i16>>) {
        let (tx, rx) = nari_pcm_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let c = cancel.clone();
        let worker = thread::spawn(move || {
            while !s.load(Ordering::SeqCst) && !c.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            drop(tx);
            released.store(true, Ordering::SeqCst);
            Ok(())
        });
        (
            PcmRecording {
                level: Arc::new(AtomicU32::new(0)),
                stop,
                cancel,
                worker: Some(worker),
            },
            rx,
        )
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn orchestration_opens_capture_during_handshake_then_releases_on_network_error() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let opened = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let o = opened.clone();
        let r = released.clone();
        let starting = tokio::task::spawn_blocking(move || {
            NariRecording::start_with(
                Arc::new(AtomicBool::new(false)),
                "test",
                &url,
                move |cancel| {
                    o.store(true, Ordering::SeqCst);
                    let capture = fake_capture(cancel, r);
                    capture.0.level.store(0.375f32.to_bits(), Ordering::Relaxed);
                    Ok(capture)
                },
            )
        });
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap();
        // Speech during the handshake must be captured, so the microphone and
        // the recording handle are both live before session.configured.
        let recording = starting.await.unwrap().unwrap();
        assert!(opened.load(Ordering::SeqCst));
        assert!(!recording.is_finished());
        ws.send(Message::Text(
            serde_json::json!({"type":"session.configured"}).to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(recording.audio_level(), 0.375);
        ws.send(Message::Text(
            serde_json::json!({"type":"error","error":{"message":"never expose this"}}).to_string(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !recording.is_finished() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(released.load(Ordering::SeqCst));
        assert_eq!(recording.audio_level(), 0.0);
        let mut finished = 0;
        while let Some(event) = recording.try_event() {
            if let NariEvent::Finished(result) = event {
                assert_eq!(result, Err(VoiceError::NariRejected));
                finished += 1;
            }
        }
        assert_eq!(finished, 1);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn orchestration_setup_error_releases_capture_and_reports_provider_error() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let released = Arc::new(AtomicBool::new(false));
        let r = released.clone();
        let starting = tokio::task::spawn_blocking(move || {
            NariRecording::start_with(Arc::new(AtomicBool::new(false)), "test", &url, move |c| {
                Ok(fake_capture(c, r))
            })
        });
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap();
        ws.send(Message::Text(
            serde_json::json!({"type":"error","error":{"code":"INSUFFICIENT_CREDITS"}}).to_string(),
        ))
        .await
        .unwrap();
        let recording = starting.await.unwrap().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(NariEvent::Finished(result)) = recording.try_event() {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(result, Err(VoiceError::NariCreditsExhausted));
        tokio::time::timeout(Duration::from_secs(1), async {
            while !released.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn filter_tail_never_exceeds_duration_cap() {
        for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
            let (tx, mut rx) = nari_pcm_channel();
            let mut c = Chunker {
                level: Arc::new(AtomicU32::new(0)),
                resampler: Resampler::new(rate).unwrap(),
                chunk: Vec::new(),
                tx,
                failed: false,
                samples: 0,
                first_callback: false,
                first_voice: false,
            };
            c.push(&vec![0.2f32; rate as usize / 5], 1);
            let cap = c.samples;
            c.finish(cap);
            let mut total = 0;
            while let Ok(chunk) = rx.try_recv() {
                total += chunk.len();
            }
            assert_eq!(total, cap, "rate {rate}");
            assert!(!c.failed);
        }
    }

    #[test]
    fn upsampling_final_frame_cannot_exceed_cap() {
        let (tx, _rx) = nari_pcm_channel();
        let cap = 16000 * MAX_RECORDING_DURATION.as_secs() as usize;
        let mut c = Chunker {
            level: Arc::new(AtomicU32::new(0)),
            resampler: Resampler::new(11025).unwrap(),
            chunk: Vec::new(),
            tx,
            failed: false,
            samples: cap - 1,
            first_callback: false,
            first_voice: false,
        };
        c.push(&[0.5f32; 100], 1);
        assert_eq!(c.samples, cap);
        assert_eq!(c.chunk.len(), 1);
    }
    #[test]
    fn construction_returns_before_microphone_or_network_are_ready() {
        // Unroutable address: the handshake cannot complete during this test.
        let (release, blocked) = mpsc::channel::<()>();
        let started = std::time::Instant::now();
        let recording = NariRecording::start_with(
            Arc::new(AtomicBool::new(false)),
            "test",
            "ws://10.255.255.1:9",
            move |c| {
                let _ = blocked.recv_timeout(Duration::from_secs(2));
                Ok(fake_capture(c, Arc::new(AtomicBool::new(false))))
            },
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(!recording.is_finished());
        assert_eq!(recording.audio_level(), 0.0);
        drop(recording);
        let _ = release.send(());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_slow_factory_finishes_cancelled_without_starting_stale_capture() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let entered = Arc::new(AtomicBool::new(false));
        let e = entered.clone();
        let stale_prevented = Arc::new(AtomicBool::new(false));
        let prevented = stale_prevented.clone();
        let (release, blocked) = mpsc::channel();
        let starting = tokio::task::spawn_blocking(move || {
            NariRecording::start_with(c, "test", &url, move |setup_cancel| {
                e.store(true, Ordering::SeqCst);
                blocked.recv_timeout(Duration::from_secs(2)).unwrap();
                prevented.store(setup_cancel.load(Ordering::SeqCst), Ordering::SeqCst);
                Err(VoiceError::Cancelled)
            })
        });
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap();
        ws.send(Message::Text(
            serde_json::json!({"type":"session.configured"}).to_string(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !entered.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        cancel.store(true, Ordering::SeqCst);
        // Construction never waits for the microphone or network.
        let recording = tokio::time::timeout(Duration::from_millis(200), starting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let result = tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if let Some(NariEvent::Finished(result)) = recording.try_event() {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(result, Err(VoiceError::Cancelled));
        tokio::time::sleep(Duration::from_millis(40)).await;
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !stale_prevented.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unified_stop_finalizes_and_drop_after_finished_preserves_token() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let released = Arc::new(AtomicBool::new(false));
        let r = released.clone();
        let starting = tokio::task::spawn_blocking(move || {
            NariRecording::start_with(c, "test", &url, move |c| Ok(fake_capture(c, r)))
        });
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap();
        ws.send(Message::Text(
            serde_json::json!({"type":"session.configured"}).to_string(),
        ))
        .await
        .unwrap();
        let recording = starting.await.unwrap().unwrap();
        recording.stop();
        let message = tokio::time::timeout(Duration::from_secs(1), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let commit: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(commit["type"], "input_audio_buffer.commit");
        ws.send(Message::Text(serde_json::json!({"type":"input_audio_buffer.commit_empty","client_event_id":commit["event_id"]}).to_string())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(NariEvent::Finished(result)) = recording.try_event() {
                    assert_eq!(result, Ok(String::new()));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        drop(recording);
        assert!(!cancel.load(Ordering::SeqCst));
        assert!(released.load(Ordering::SeqCst));
    }
}
