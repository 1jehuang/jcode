//! Dedicated-thread native streaming capture and nonblocking desktop handle.
use super::{resample::Resampler, *};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

/// Native mono 16k PCM capture. Stop is a signal, EOF follows buffered chunks.
/// Dropping an unfinished handle cancels, never blocks the UI on native teardown.
pub struct PcmRecording {
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
        let (tx, rx) = nari_pcm_channel();
        let (ready, started) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let (c, s) = (cancel.clone(), stop.clone());
        let worker = thread::Builder::new()
            .name("voice-pcm".into())
            .spawn(move || {
                let result = capture(tx, &ready, c, s);
                if let Err(e) = &result {
                    let _ = ready.try_send(Err(e.clone()));
                }
                result
            })
            .map_err(|_| VoiceError::CaptureFailed)?;
        match wait_started(&started, &cancel) {
            Ok(Ok(())) => Ok((
                Self {
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
    resampler: Resampler,
    chunk: Vec<i16>,
    tx: tokio::sync::mpsc::Sender<Vec<i16>>,
    failed: bool,
    samples: usize,
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
        for frame in data.chunks_exact(channels) {
            if self.samples >= 16000 * MAX_RECORDING_DURATION.as_secs() as usize {
                return;
            }
            let mono = frame
                .iter()
                .map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s))
                .sum::<f32>()
                / channels as f32;
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
                    return;
                }
            }
        }
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
) -> Result<(), VoiceError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(VoiceError::Cancelled);
    }
    let device = cpal::default_host()
        .default_input_device()
        .ok_or(VoiceError::MicrophoneUnavailable)?;
    let supported = device
        .default_input_config()
        .map_err(|_| VoiceError::MicrophoneUnavailable)?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    if config.channels == 0 {
        return Err(VoiceError::MicrophoneUnavailable);
    }
    let state = Arc::new(Mutex::new(Chunker {
        resampler: Resampler::new(config.sample_rate.0)?,
        chunk: Vec::with_capacity(1602),
        tx,
        failed: false,
        samples: 0,
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
    stream
        .play()
        .map_err(|_| VoiceError::MicrophoneUnavailable)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(VoiceError::Cancelled);
    }
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
/// Unified network + microphone operation. Constructors block, all handle methods
/// and Drop are nonblocking. Capture begins only after session.configured.
pub struct NariRecording {
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    events: Arc<Mutex<Events>>,
    worker: thread::JoinHandle<()>,
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
        let (ready, started) = mpsc::sync_channel(1);
        let (c, s, e) = (cancel.clone(), stop.clone(), events.clone());
        let worker = thread::Builder::new()
            .name("voice-nari".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| VoiceError::CaptureFailed)?;
                    let result = runtime.block_on(async {
                        let mut session = NariSession::connect_to(&url, &key, c.clone()).await?;
                        drop(key);
                        let setup_cancel = Arc::new(AtomicBool::new(false));
                        let factory_cancel = setup_cancel.clone();
                        let (mic, pcm) = tokio::select! {
                            biased;
                            _ = super::nari::cancelled(&c) => { setup_cancel.store(true, Ordering::SeqCst); return Err(VoiceError::Cancelled); },
                            error = session.wait_for_error() => { setup_cancel.store(true, Ordering::SeqCst); return Err(error); },
                            result = tokio::task::spawn_blocking(move || factory(factory_cancel)) => result.map_err(|_|VoiceError::CaptureFailed)??,
                        };
                        let _ = ready.send(Ok(()));
                        let mic_stop = mic.stop.clone();
                        let forward_stop = async {
                            loop {
                                if s.load(Ordering::SeqCst) {
                                    mic_stop.store(true, Ordering::SeqCst);
                                    return;
                                }
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        };
                        let stream = session.run(pcm, |event| {
                            if !matches!(event, NariEvent::Finished(_)) {
                                if let Ok(mut events) = e.lock() {
                                    events.push(event);
                                }
                            }
                        });
                        tokio::pin!(stream);
                        tokio::pin!(forward_stop);
                        let result =
                            tokio::select! {r=&mut stream=>r,_=&mut forward_stop=>stream.await};
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
                if let Err(err) = &result {
                    let _ = ready.try_send(Err(err.clone()));
                }
                if let Ok(mut events) = e.lock() {
                    events.push(NariEvent::Finished(result));
                }
            })
            .map_err(|_| VoiceError::CaptureFailed)?;
        match wait_started(&started, &cancel) {
            Ok(Ok(())) => Ok(Self {
                cancel,
                stop,
                events,
                worker,
            }),
            Ok(Err(e)) => Err(e),
            _ => Err(VoiceError::CaptureFailed),
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
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
            resampler: Resampler::new(48000).unwrap(),
            chunk: Vec::new(),
            tx,
            failed: false,
            samples: 0,
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
                stop,
                cancel,
                worker: Some(worker),
            },
            rx,
        )
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn orchestration_waits_for_configured_then_releases_on_network_error() {
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
                    Ok(fake_capture(cancel, r))
                },
            )
        });
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!opened.load(Ordering::SeqCst));
        ws.send(Message::Text(
            serde_json::json!({"type":"session.configured"}).to_string(),
        ))
        .await
        .unwrap();
        let recording = starting.await.unwrap().unwrap();
        assert!(opened.load(Ordering::SeqCst));
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
    async fn orchestration_setup_error_never_opens_capture() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let starting = tokio::task::spawn_blocking(move || {
            NariRecording::start_with(Arc::new(AtomicBool::new(false)), "test", &url, |_| {
                panic!("must not open capture")
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
        assert!(matches!(
            starting.await.unwrap(),
            Err(VoiceError::NariCreditsExhausted)
        ));
    }
    #[test]
    fn filter_tail_never_exceeds_duration_cap() {
        for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
            let (tx, mut rx) = nari_pcm_channel();
            let mut c = Chunker {
                resampler: Resampler::new(rate).unwrap(),
                chunk: Vec::new(),
                tx,
                failed: false,
                samples: 0,
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
            resampler: Resampler::new(11025).unwrap(),
            chunk: Vec::new(),
            tx,
            failed: false,
            samples: cap - 1,
        };
        c.push(&[0.5f32; 100], 1);
        assert_eq!(c.samples, cap);
        assert_eq!(c.chunk.len(), 1);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_slow_factory_does_not_block_constructor_or_start_stale_capture() {
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
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(200), starting)
                .await
                .unwrap()
                .unwrap(),
            Err(VoiceError::Cancelled)
        ));
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
