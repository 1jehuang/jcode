//! Native Nari streaming and subscription-backed WAV transcription.
//!
//! Capability checks never open a microphone. Call `MicrophoneRecording::start` only
//! after an explicit user action. Capture is opt-in via the `voice-capture` feature.
//! `NariRecording` combines confirmed network setup and native capture for desktop
//! clients. `NariSession` and `nari_pcm_channel` accept bounded mono 16 kHz PCM16
//! from any source without the capture feature. Nari requires a caller-supplied
//! provider key. The existing subscription APIs still need no provider key.
use crate::{subscription_api, subscription_catalog};
use serde::Deserialize;
use std::{fmt, time::Duration};

mod nari;
pub mod timing;
pub use nari::{
    NARI_PCM_CHUNK_SAMPLES, NARI_USD_PER_AUDIO_HOUR, NariEvent, NariSession, correct_transcript,
    estimated_transcription_usd, nari_api_key, nari_pcm_channel, recognition_prompt,
};
#[cfg(any(feature = "voice-capture", test))]
mod resample;
#[cfg(feature = "voice-capture")]
mod streaming_capture;
#[cfg(feature = "voice-capture")]
pub use streaming_capture::{NariRecording, PcmRecording};

pub const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_RECORDING_DURATION: Duration = Duration::from_secs(5 * 60);
pub const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Deliberately retains no URLs, credentials, provider errors, or response bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceError {
    NariNotConfigured,
    NariCreditsExhausted,
    NariRejected,
    NotConfigured,
    InvalidAudio,
    InvalidLanguage,
    Unavailable,
    Timeout,
    Network,
    Http(u16),
    InvalidResponse,
    ResponseTooLarge,
    MicrophoneUnavailable,
    CaptureFailed,
    Cancelled,
}
impl fmt::Display for VoiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NariNotConfigured => {
                f.write_str("Configure a valid Nari API key to use voice transcription")
            }
            Self::NariCreditsExhausted => {
                f.write_str("Nari credits are exhausted. Add credits before retrying")
            }
            Self::NariRejected => f.write_str(
                "Nari rejected the voice session. Check your key, credits, and settings",
            ),
            Self::NotConfigured => f.write_str("Sign in to Jcode to use voice transcription"),
            Self::InvalidAudio => {
                f.write_str("Audio must be a nonempty mono PCM16 WAV, at most 5 minutes and 10 MiB")
            }
            Self::InvalidLanguage => f.write_str("Invalid transcription language code"),
            Self::Unavailable => {
                f.write_str("Voice transcription is unavailable for this subscription")
            }
            Self::Timeout => f.write_str("Voice transcription request timed out"),
            Self::Network => f.write_str("Unable to reach the voice transcription service"),
            Self::Http(status) => write!(f, "Voice transcription service returned HTTP {status}"),
            Self::InvalidResponse => f.write_str("Invalid voice transcription response"),
            Self::ResponseTooLarge => {
                f.write_str("Voice transcription response exceeds the size limit")
            }
            Self::MicrophoneUnavailable => {
                f.write_str("No supported microphone is available. Check microphone permissions")
            }
            Self::CaptureFailed => f.write_str("Microphone recording failed"),
            Self::Cancelled => f.write_str("Microphone recording was cancelled"),
        }
    }
}
impl std::error::Error for VoiceError {}

fn network_error(error: reqwest::Error) -> VoiceError {
    if error.is_timeout() {
        VoiceError::Timeout
    } else {
        VoiceError::Network
    }
}

fn client() -> Result<reqwest::Client, VoiceError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(TRANSCRIPTION_TIMEOUT)
        .build()
        .map_err(network_error)
}

async fn response_bytes(mut response: reqwest::Response) -> Result<Vec<u8>, VoiceError> {
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 | 404 => VoiceError::Unavailable,
            status => VoiceError::Http(status),
        });
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(VoiceError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
            return Err(VoiceError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Deserialize, Default)]
struct CapabilityResponse {
    #[serde(default)]
    capabilities: subscription_api::SubscriptionCapabilities,
}

/// Fail closed for missing keys, old servers, offline errors, and denied requests.
/// This does not capture audio, request microphone access, or cache account secrets.
pub async fn subscription_voice_available() -> bool {
    let Some(key) = subscription_catalog::configured_api_key().filter(|key| !key.trim().is_empty())
    else {
        return false;
    };
    let Ok(client) = client() else { return false };
    voice_available_with(&client, &subscription_api::configured_api_base(), &key)
        .await
        .unwrap_or(false)
}

async fn voice_available_with(
    client: &reqwest::Client,
    base: &str,
    key: &str,
) -> Result<bool, VoiceError> {
    let response = client
        .get(format!("{}/me", base.trim_end_matches('/')))
        .bearer_auth(key)
        .timeout(subscription_api::ME_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(network_error)?;
    let me: CapabilityResponse = serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| VoiceError::InvalidResponse)?;
    Ok(me.capabilities.voice_transcription)
}

/// Upload one bounded in-memory WAV using the configured Jcode subscription key.
/// The server is authoritative about entitlement, so no preflight check is required.
/// `language`, if supplied, is a lowercase ISO 639-1 code (for example `en`).
/// Dropping the future cancels the request. Neither audio nor transcripts are logged.
pub async fn transcribe_wav(wav: Vec<u8>, language: Option<&str>) -> Result<String, VoiceError> {
    let key = subscription_catalog::configured_api_key()
        .filter(|key| !key.trim().is_empty())
        .ok_or(VoiceError::NotConfigured)?;
    transcribe_with(
        &client()?,
        &subscription_api::configured_api_base(),
        &key,
        wav,
        language,
        TRANSCRIPTION_TIMEOUT,
    )
    .await
}

async fn transcribe_with(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    wav: Vec<u8>,
    language: Option<&str>,
    timeout: Duration,
) -> Result<String, VoiceError> {
    validate_wav(&wav)?;
    let mut form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(wav)
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .map_err(|_| VoiceError::InvalidAudio)?,
    );
    if let Some(language) = language {
        if language.len() != 2 || !language.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(VoiceError::InvalidLanguage);
        }
        form = form.text("language", language.to_owned());
    }
    let response = client
        .post(format!(
            "{}/audio/transcriptions",
            base.trim_end_matches('/')
        ))
        .bearer_auth(key)
        .multipart(form)
        .timeout(timeout)
        .send()
        .await
        .map_err(network_error)?;
    #[derive(Deserialize)]
    struct Transcript {
        text: String,
    }
    let transcript: Transcript = serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| VoiceError::InvalidResponse)?;
    Ok(nari::correct_transcript(&transcript.text))
}

/// Validate the PCM16 mono WAV format emitted by capture, including duration.
fn validate_wav(bytes: &[u8]) -> Result<(), VoiceError> {
    let invalid = || VoiceError::InvalidAudio;
    if bytes.len() < 44
        || bytes.len() > MAX_AUDIO_BYTES
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
    {
        return Err(invalid());
    }
    let u32_at = |pos| u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
    if u32_at(4) as usize != bytes.len() - 8 {
        return Err(invalid());
    }
    let mut pos = 12;
    let mut rate = None;
    let mut data_len = None;
    while pos + 8 <= bytes.len() {
        let len = u32_at(pos + 4) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(invalid)?;
        match &bytes[pos..pos + 4] {
            b"fmt " => {
                if rate.is_some()
                    || len < 16
                    || bytes[start..start + 4] != [1, 0, 1, 0]
                    || bytes[start + 12..start + 16] != [2, 0, 16, 0]
                {
                    return Err(invalid());
                }
                let sample_rate = u32_at(start + 4);
                if !(8000..=192000).contains(&sample_rate) || u32_at(start + 8) != sample_rate * 2 {
                    return Err(invalid());
                }
                rate = Some(sample_rate);
            }
            b"data" => {
                if data_len.is_some() || len == 0 || !len.is_multiple_of(2) {
                    return Err(invalid());
                }
                data_len = Some(len);
            }
            _ => {}
        }
        pos = end + len % 2;
    }
    if pos != bytes.len()
        || data_len.ok_or_else(invalid)? as u64
            > rate.ok_or_else(invalid)? as u64 * 2 * MAX_RECORDING_DURATION.as_secs()
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(any(feature = "voice-capture", test))]
fn encode_wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>, VoiceError> {
    let len = samples
        .len()
        .checked_mul(2)
        .filter(|len| *len <= MAX_AUDIO_BYTES - 44)
        .ok_or(VoiceError::InvalidAudio)? as u32;
    let mut wav = Vec::with_capacity(44 + len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    validate_wav(&wav)?;
    Ok(wav)
}

#[cfg(feature = "voice-capture")]
pub use capture::MicrophoneRecording;

#[cfg(feature = "voice-capture")]
mod capture {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    enum Command {
        Stop,
        Cancel,
    }
    /// Owns a native microphone stream on a dedicated thread. No temporary files.
    /// Start only on explicit user interaction. Stop, cancel, drop, device failure,
    /// the byte cap, or the five-minute deadline releases the microphone.
    pub struct MicrophoneRecording {
        command: mpsc::Sender<Command>,
        cancel: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<Result<Vec<u8>, VoiceError>>>,
    }
    impl MicrophoneRecording {
        pub fn start() -> Result<Self, VoiceError> {
            Self::start_cancellable(Arc::new(AtomicBool::new(false)))
        }
        /// Set `cancel` to true when the initiating UI action becomes stale.
        /// This is checked after native permission/setup calls before starting input.
        /// Run this blocking constructor off the UI thread.
        pub fn start_cancellable(cancel: Arc<AtomicBool>) -> Result<Self, VoiceError> {
            if cancel.load(Ordering::SeqCst) {
                return Err(VoiceError::Cancelled);
            }
            let worker_cancel = cancel.clone();
            let (command, rx) = mpsc::channel();
            let (ready, started) = mpsc::sync_channel(1);
            let worker = std::thread::spawn(move || {
                let result = run(rx, &ready, worker_cancel);
                // If setup failed, return a redacted setup error to start().
                if let Err(error) = &result {
                    let _ = ready.try_send(Err(error.clone()));
                }
                result
            });
            match started.recv() {
                Ok(Ok(())) => Ok(Self {
                    command,
                    cancel,
                    worker: Some(worker),
                }),
                Ok(Err(error)) => {
                    let _ = worker.join();
                    Err(error)
                }
                _ => {
                    let _ = worker.join();
                    Err(VoiceError::MicrophoneUnavailable)
                }
            }
        }
        pub fn is_finished(&self) -> bool {
            self.worker
                .as_ref()
                .is_none_or(|worker| worker.is_finished())
        }
        /// Finish capture and return its WAV without cancelling the caller's
        /// shared operation token. Any cancellation already requested is preserved.
        pub fn stop(mut self) -> Result<Vec<u8>, VoiceError> {
            let _ = self.command.send(Command::Stop);
            self.worker
                .take()
                .ok_or(VoiceError::CaptureFailed)?
                .join()
                .map_err(|_| VoiceError::CaptureFailed)?
        }
        pub fn cancel(self) {
            drop(self);
        }
    }
    impl Drop for MicrophoneRecording {
        fn drop(&mut self) {
            // stop() takes and joins the worker itself. Its consumed handle must
            // not cancel the shared recording/transcription operation afterward.
            if let Some(worker) = self.worker.take() {
                self.cancel.store(true, Ordering::SeqCst);
                let _ = self.command.send(Command::Cancel);
                let _ = worker.join();
            }
        }
    }

    struct Buffer {
        samples: Vec<i16>,
        failed: bool,
        full: bool,
    }
    fn run(
        command: mpsc::Receiver<Command>,
        ready: &mpsc::SyncSender<Result<(), VoiceError>>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<u8>, VoiceError> {
        if cancelled.load(Ordering::SeqCst) {
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
        let rate = config.sample_rate.0;
        if config.channels == 0 || !(8000..=192000).contains(&rate) {
            return Err(VoiceError::MicrophoneUnavailable);
        }
        let max_samples = ((rate as u64 * MAX_RECORDING_DURATION.as_secs()) as usize)
            .min((MAX_AUDIO_BYTES - 44) / 2);
        let buffer = Arc::new(Mutex::new(Buffer {
            samples: Vec::with_capacity(max_samples),
            failed: false,
            full: false,
        }));
        macro_rules! build {
            ($ty:ty) => {
                build::<$ty>(&device, &config, buffer.clone(), max_samples)?
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
        if cancelled.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        stream
            .play()
            .map_err(|_| VoiceError::MicrophoneUnavailable)?;
        if cancelled.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        let deadline = std::time::Instant::now() + MAX_RECORDING_DURATION;
        let _ = ready.send(Ok(()));
        let mut cancel = false;
        loop {
            if cancelled.load(Ordering::SeqCst) {
                cancel = true;
                break;
            }
            match command.recv_timeout(Duration::from_millis(20)) {
                Ok(Command::Stop) => break,
                Ok(Command::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    cancel = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let state = buffer.lock().map_err(|_| VoiceError::CaptureFailed)?;
            if state.failed || state.full || std::time::Instant::now() >= deadline {
                break;
            }
        }
        drop(stream);
        let state = buffer.lock().map_err(|_| VoiceError::CaptureFailed)?;
        if cancel || cancelled.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        if state.failed {
            return Err(VoiceError::CaptureFailed);
        }
        encode_wav(&state.samples, rate)
    }

    fn append_samples<T>(state: &mut Buffer, data: &[T], channels: usize, max_samples: usize)
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        for frame in data.chunks_exact(channels) {
            if state.samples.len() == max_samples {
                break;
            }
            let mono = frame
                .iter()
                .map(|sample| <f32 as cpal::FromSample<T>>::from_sample_(*sample))
                .sum::<f32>()
                / channels as f32;
            state
                .samples
                .push((mono.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16);
        }
        state.full = state.samples.len() == max_samples;
    }

    fn build<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        buffer: Arc<Mutex<Buffer>>,
        max_samples: usize,
    ) -> Result<cpal::Stream, VoiceError>
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let errors = buffer.clone();
        let channels = config.channels as usize;
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    let Ok(mut state) = buffer.lock() else { return };
                    append_samples(&mut state, data, channels, max_samples);
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn downmix_and_cap_without_microphone_access() {
            let mut state = Buffer {
                samples: Vec::new(),
                full: false,
                failed: false,
            };
            append_samples(&mut state, &[1.0f32, -1.0, 0.5, 0.5, 1.0, 1.0], 2, 2);
            assert_eq!(state.samples, [0, 16384]);
            assert!(state.full);
            append_samples(&mut state, &[1.0f32, 1.0], 2, 2);
            assert_eq!(state.samples.len(), 2);
        }

        fn fake_recording(
            expect_stop: bool,
        ) -> (MicrophoneRecording, Arc<std::sync::atomic::AtomicBool>) {
            let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = completed.clone();
            let (command, rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = match (
                    rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                    expect_stop,
                ) {
                    (Command::Stop, true) => Ok(vec![42]),
                    (Command::Cancel, false) => Ok(Vec::new()),
                    _ => panic!("wrong capture command"),
                };
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                result
            });
            (
                MicrophoneRecording {
                    command,
                    cancel: Arc::new(AtomicBool::new(false)),
                    worker: Some(worker),
                },
                completed,
            )
        }

        #[test]
        fn cancelled_start_never_opens_a_device() {
            assert!(matches!(
                MicrophoneRecording::start_cancellable(Arc::new(AtomicBool::new(true))),
                Err(VoiceError::Cancelled)
            ));
        }

        #[test]
        fn stop_joins_and_returns_audio() {
            let (recording, completed) = fake_recording(true);
            let cancel = recording.cancel.clone();
            assert_eq!(recording.stop().unwrap(), [42]);
            assert!(completed.load(Ordering::SeqCst));
            assert!(
                !cancel.load(Ordering::SeqCst),
                "successful stop must allow transcription"
            );
        }

        #[test]
        fn cancel_and_drop_signal_worker_and_join() {
            // Dedicated fake worker checks the command without opening a device.
            let (recording, completed) = fake_recording(false);
            let cancel = recording.cancel.clone();
            recording.cancel();
            assert!(cancel.load(Ordering::SeqCst));
            assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
            let (recording, completed) = fake_recording(false);
            let cancel = recording.cancel.clone();
            drop(recording);
            assert!(cancel.load(Ordering::SeqCst));
            assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn mock(
        status: u16,
        headers: &str,
        body: Vec<u8>,
        delay: Duration,
    ) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let headers = headers.to_owned();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut block = [0; 4096];
            loop {
                let n = stream.read(&mut block).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&block[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap_or("0")
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            std::thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body);
            request
        });
        (base, worker)
    }
    fn wav() -> Vec<u8> {
        encode_wav(&[0, -100, 100, i16::MAX], 16000).unwrap()
    }
    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }
    async fn upload(
        base: &str,
        language: Option<&str>,
        timeout: Duration,
    ) -> Result<String, VoiceError> {
        transcribe_with(
            &test_client(),
            base,
            "jcode-secret-key",
            wav(),
            language,
            timeout,
        )
        .await
    }

    #[tokio::test]
    async fn configured_public_api_requires_key_and_uses_configured_base() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        struct Restore(Vec<(&'static str, Option<String>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    match value {
                        Some(value) => crate::env::set_var(key, value),
                        None => crate::env::remove_var(key),
                    }
                }
            }
        }
        let _restore = Restore(
            ["JCODE_API_KEY", "JCODE_API_BASE"]
                .into_iter()
                .map(|key| (key, std::env::var(key).ok()))
                .collect(),
        );
        crate::env::remove_var("JCODE_API_KEY");
        crate::env::set_var("JCODE_API_BASE", "http://127.0.0.1:1");
        assert!(!subscription_voice_available().await);
        assert_eq!(
            transcribe_wav(wav(), None).await,
            Err(VoiceError::NotConfigured)
        );
        crate::env::set_var("JCODE_API_KEY", "public-api-test-key");
        let (base, worker) = mock(
            200,
            "",
            br#"{"capabilities":{"voice_transcription":true}}"#.to_vec(),
            Duration::ZERO,
        );
        crate::env::set_var("JCODE_API_BASE", &base);
        assert!(subscription_voice_available().await);
        assert!(
            String::from_utf8(worker.join().unwrap())
                .unwrap()
                .contains("authorization: Bearer public-api-test-key")
        );
        let (base, worker) = mock(
            200,
            "",
            br#"{"text":"configured upload"}"#.to_vec(),
            Duration::ZERO,
        );
        crate::env::set_var("JCODE_API_BASE", &base);
        assert_eq!(
            transcribe_wav(wav(), None).await.unwrap(),
            "configured upload"
        );
        assert!(
            String::from_utf8_lossy(&worker.join().unwrap())
                .contains("authorization: Bearer public-api-test-key")
        );
    }

    #[tokio::test]
    async fn multipart_and_bearer_auth_match_backend_contract() {
        let (base, worker) = mock(
            200,
            "Content-Type: application/json\r\n",
            br#"{"text":"Hello from voice"}"#.to_vec(),
            Duration::ZERO,
        );
        assert_eq!(
            upload(&base, Some("en"), Duration::from_secs(2))
                .await
                .unwrap(),
            "Hello from voice"
        );
        let request = worker.join().unwrap();
        let text = String::from_utf8_lossy(&request);
        assert!(text.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"));
        assert!(text.contains("authorization: Bearer jcode-secret-key\r\n"));
        assert!(text.contains("multipart/form-data; boundary="));
        assert!(text.contains("name=\"file\"; filename=\"recording.wav\""));
        assert!(text.contains("Content-Type: audio/wav"));
        assert!(text.contains("name=\"language\"\r\n\r\nen\r\n"));
        assert!(request.windows(wav().len()).any(|part| part == wav()));
        assert!(!text.contains("groq"));
        assert_eq!(text.matches("jcode-secret-key").count(), 1);
    }

    #[tokio::test]
    async fn language_is_optional() {
        let (base, worker) = mock(200, "", br#"{"text":""}"#.to_vec(), Duration::ZERO);
        assert_eq!(
            upload(&base, None, Duration::from_secs(2)).await.unwrap(),
            ""
        );
        assert!(!String::from_utf8_lossy(&worker.join().unwrap()).contains("name=\"language\""));
    }

    #[tokio::test]
    async fn capabilities_default_false_and_use_bearer_header() {
        for (body, expected) in [
            (r#"{"capabilities":{"voice_transcription":true}}"#, true),
            (r#"{"capabilities":{"voice_transcription":false}}"#, false),
            (r#"{"capabilities":{}}"#, false),
            (
                r#"{"account_id":"old-account","tier":"plus","status":"active"}"#,
                false,
            ),
        ] {
            let (base, worker) = mock(200, "", body.as_bytes().to_vec(), Duration::ZERO);
            assert_eq!(
                voice_available_with(&test_client(), &base, "secret")
                    .await
                    .unwrap(),
                expected
            );
            let request = String::from_utf8(worker.join().unwrap()).unwrap();
            assert!(request.starts_with("GET /v1/me HTTP/1.1\r\n"));
            assert!(request.contains("authorization: Bearer secret\r\n"));
        }
    }

    #[tokio::test]
    async fn failures_never_expose_credentials_or_server_bodies() {
        for (status, body, expected) in [
            (401, "jcode-secret-key", VoiceError::Unavailable),
            (403, "groq-secret-key", VoiceError::Unavailable),
            (404, "legacy", VoiceError::Unavailable),
            (429, "groq-secret-key", VoiceError::Http(429)),
            (500, "jcode-secret-key", VoiceError::Http(500)),
            (200, "groq-secret-key", VoiceError::InvalidResponse),
        ] {
            let (base, worker) = mock(status, "", body.as_bytes().to_vec(), Duration::ZERO);
            let error = upload(&base, None, Duration::from_secs(2))
                .await
                .unwrap_err();
            assert_eq!(error, expected);
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("secret-key"));
            assert!(!rendered.contains(&base));
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn requests_time_out_and_redirects_are_not_followed() {
        let (base, worker) = mock(
            200,
            "",
            br#"{"text":"late"}"#.to_vec(),
            Duration::from_millis(100),
        );
        assert_eq!(
            upload(&base, None, Duration::from_millis(20))
                .await
                .unwrap_err(),
            VoiceError::Timeout
        );
        worker.join().unwrap();
        let (base, worker) = mock(
            307,
            "Location: http://127.0.0.1:1/secret\r\n",
            Vec::new(),
            Duration::ZERO,
        );
        assert_eq!(
            upload(&base, None, Duration::from_secs(2))
                .await
                .unwrap_err(),
            VoiceError::Http(307)
        );
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn oversized_response_is_rejected() {
        let (base, worker) = mock(200, "", vec![b' '; MAX_RESPONSE_BYTES + 1], Duration::ZERO);
        assert_eq!(
            upload(&base, None, Duration::from_secs(2))
                .await
                .unwrap_err(),
            VoiceError::ResponseTooLarge
        );
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn invalid_input_is_rejected_before_network() {
        let client = test_client();
        assert_eq!(
            transcribe_with(
                &client,
                "http://127.0.0.1:1",
                "secret",
                vec![],
                None,
                Duration::from_secs(1)
            )
            .await,
            Err(VoiceError::InvalidAudio)
        );
        assert_eq!(
            upload(
                "http://127.0.0.1:1",
                Some("en\r\nsecret"),
                Duration::from_secs(1)
            )
            .await,
            Err(VoiceError::InvalidLanguage)
        );
        let error = upload(
            "http://127.0.0.1:1/private-secret",
            None,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error, VoiceError::Network);
        assert!(!format!("{error:?} {error}").contains("private-secret"));
    }

    #[test]
    fn wav_encoding_and_bounds() {
        let bytes = wav();
        assert_eq!(bytes.len(), 52);
        assert_eq!(&bytes[44..], &[0, 0, 156, 255, 100, 0, 255, 127]);
        assert!(validate_wav(&bytes).is_ok());
        for end in 0..bytes.len() {
            assert_eq!(validate_wav(&bytes[..end]), Err(VoiceError::InvalidAudio));
        }
        assert_eq!(encode_wav(&[], 16000), Err(VoiceError::InvalidAudio));
        assert!(encode_wav(&vec![0; 8000 * 300], 8000).is_ok());
        assert_eq!(
            encode_wav(&vec![0; 8000 * 300 + 1], 8000),
            Err(VoiceError::InvalidAudio)
        );
        assert_eq!(
            encode_wav(&vec![0; MAX_AUDIO_BYTES / 2], 48000),
            Err(VoiceError::InvalidAudio)
        );
        let mut bad = bytes.clone();
        bad[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(validate_wav(&bad), Err(VoiceError::InvalidAudio));
        let mut bad = bytes;
        bad[22] = 2;
        assert_eq!(validate_wav(&bad), Err(VoiceError::InvalidAudio));
    }
}
