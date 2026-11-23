//! Cancellable client-side speech playback. Voice models remain external.
//! Supports an explicitly configured speech HTTP API or local eSpeak fallback.
use std::{
    collections::VecDeque,
    fmt,
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(feature = "desktop-config")]
mod http;

pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_QUEUED_UTTERANCES: usize = 16;
const POLL: Duration = Duration::from_millis(10);
const MAX_PLAYBACK: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechError {
    Unavailable,
    InvalidText,
    QueueFull,
    InvalidEndpoint,
    PlayerUnavailable,
    WorkerUnavailable,
    InvalidConfiguration,
}
impl fmt::Display for SpeechError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Install espeak-ng or espeak to enable local speech",
            Self::InvalidText => "Speech must be nonempty, NUL-free, and at most 4096 bytes",
            Self::QueueFull => "Speech queue is full",
            Self::InvalidEndpoint => {
                "Speech endpoint must be HTTP(S), without credentials or a fragment"
            }
            Self::PlayerUnavailable => "Install pw-play or aplay to play HTTP speech",
            Self::WorkerUnavailable => "Speech worker is unavailable",
            Self::InvalidConfiguration => {
                "Speech configuration is invalid or its API-key environment variable is missing"
            }
        })
    }
}
impl std::error::Error for SpeechError {}

type Backend = Arc<dyn Fn(&str) -> std::io::Result<Child> + Send + Sync>;
#[derive(Default)]
struct State {
    queue: VecDeque<(String, Option<PlaybackTrace>)>,
    active: Option<(Child, Instant)>,
    generation: u64,
    synthesizing: bool,
    awaiting_quiet: bool,
    authorized: bool,
    last_error: Option<&'static str>,
    stopped: bool,
}

pub struct PlaybackTrace {
    pub session: String,
    pub utterance: u64,
}
impl PlaybackTrace {
    #[cfg(not(feature = "desktop-config"))]
    pub fn mark(&self, _stage: &'static str, _held_ms: u128) {}
    #[cfg(feature = "desktop-config")]
    pub fn mark(&self, stage: &'static str, held_ms: u128) {
        jcode_base::logging::info(&format!(
            "JARVIS_PLAYBACK session={} utterance={} stage={stage} monotonic_ms={} held_ms={held_ms}",
            self.session,
            self.utterance,
            crate::timing::monotonic_ms()
        ));
    }
}
impl State {
    fn stop_child(&mut self) {
        if let Some((mut child, _)) = self.active.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Called under the player mutex, immediately before starting a gated player.
/// Condvar waits release the mutex so the TUI can authorize or cancel safely.
fn await_quiet<'a>(
    mut state: std::sync::MutexGuard<'a, State>,
    wake: &Condvar,
    generation: u64,
    gated: bool,
) -> (std::sync::MutexGuard<'a, State>, bool) {
    if !gated {
        let ok = !state.stopped && state.generation == generation;
        return (state, ok);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    state.awaiting_quiet = true;
    while !state.stopped && state.generation == generation && !state.authorized {
        let now = Instant::now();
        if now >= deadline {
            state.last_error = Some("Speech expired while waiting for a quiet microphone");
            break;
        }
        state = wake
            .wait_timeout(state, deadline.duration_since(now))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    let ok = !state.stopped && state.generation == generation && state.authorized;
    state.awaiting_quiet = false;
    state.authorized = false;
    (state, ok)
}
impl Drop for State {
    fn drop(&mut self) {
        self.stop_child();
    }
}

/// A bounded FIFO serviced by a dedicated thread. Enqueue never waits for speech
/// to finish. Playback failures discard that utterance and continue the queue.
/// Cancellation is reusable and takes effect before any subsequent enqueue.
/// Drop cancels playback, reaps the child, and joins the worker.
pub struct SpeechPlayer {
    shared: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}
impl SpeechPlayer {
    /// Detect an installed backend by invoking its silent version command.
    /// Detection is synchronous, with a one-second deadline per candidate.
    pub fn new() -> Result<Self, SpeechError> {
        for executable in ["espeak-ng", "espeak"] {
            if probe(executable) {
                return Self::with_backend(Arc::new(move |text| {
                    Command::new(executable)
                        .args(["-b", "1"])
                        // A leading space makes arbitrary text a positional argument,
                        // even when the supplied utterance starts with an option.
                        .arg(format!(" {text}"))
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                }));
            }
        }
        Err(SpeechError::Unavailable)
    }

    #[cfg(feature = "desktop-config")]
    pub fn with_endpoint(endpoint: &str) -> Result<Self, SpeechError> {
        Self::with_http(endpoint, "piper-jarvis", "jarvis", None)
    }

    /// Use an external OpenAI-compatible speech endpoint. Secrets are kept out
    /// of URLs, model context, logs, and error messages.
    #[cfg(feature = "desktop-config")]
    pub fn with_http(
        endpoint: &str,
        model: &str,
        voice: &str,
        api_key: Option<String>,
    ) -> Result<Self, SpeechError> {
        http::create(endpoint, model, voice, api_key)
    }

    fn with_backend(backend: Backend) -> Result<Self, SpeechError> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let state = shared.clone();
        let worker = thread::Builder::new()
            .name("jcode-speech".into())
            .spawn(move || run(state, backend))
            .map_err(|_| SpeechError::WorkerUnavailable)?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Queue one utterance without waiting for playback or queue capacity.
    /// Caps are in UTF-8 bytes and exclude the single active utterance.
    pub fn enqueue(&self, text: &str) -> Result<(), SpeechError> {
        self.enqueue_inner(text, None)
    }

    /// Prepare audio without suppressing the microphone. A prepared reply
    /// requires an explicit `authorize_ready` before it can play.
    pub fn enqueue_gated(&self, text: &str, trace: PlaybackTrace) -> Result<(), SpeechError> {
        self.enqueue_inner(text, Some(trace))
    }

    fn enqueue_inner(&self, text: &str, trace: Option<PlaybackTrace>) -> Result<(), SpeechError> {
        if text.len() > MAX_TEXT_BYTES || text.trim().is_empty() || text.contains('\0') {
            return Err(SpeechError::InvalidText);
        }
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().map_err(|_| SpeechError::WorkerUnavailable)?;
        if state.stopped || self.worker.as_ref().is_none_or(|w| w.is_finished()) {
            return Err(SpeechError::WorkerUnavailable);
        }
        if state.queue.len() >= MAX_QUEUED_UTTERANCES {
            return Err(SpeechError::QueueFull);
        }
        state.queue.push_back((text.to_owned(), trace));
        wake.notify_one();
        Ok(())
    }

    /// True only when neither synthesis nor playback nor queued work remains.
    pub fn is_idle(&self) -> bool {
        let state = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        state.queue.is_empty() && state.active.is_none() && !state.synthesizing
    }

    /// Synthesis and waiting for a quiet mic never suppress capture.
    pub fn owns_audio_output(&self) -> bool {
        let state = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        state.active.is_some() || state.authorized
    }

    /// Claim the mic only when audio is ready. The worker starts no child
    /// until this succeeds. Its spawn is serialized with cancel on this mutex.
    pub fn authorize_ready(&self, claim: impl FnOnce() -> bool) -> bool {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if state.awaiting_quiet && !state.authorized && !state.stopped && claim() {
            state.authorized = true;
            wake.notify_all();
            return true;
        }
        false
    }

    /// A redacted asynchronous failure, for the local UI rather than model context.
    pub fn take_error(&self) -> Option<&'static str> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_error
            .take()
    }

    /// Clear pending speech and kill and reap the active process, if any.
    pub fn cancel(&self) {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.awaiting_quiet = false;
        state.authorized = false;
        state.queue.clear();
        state.stop_child();
        wake.notify_one();
    }
}
impl Drop for SpeechPlayer {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        {
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            state.queue.clear();
            state.stop_child();
            wake.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(shared: Arc<(Mutex<State>, Condvar)>, backend: Backend) {
    let (lock, wake) = &*shared;
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if state.stopped {
            break;
        }
        if let Some((child, started)) = state.active.as_mut() {
            match child.try_wait() {
                Ok(None) if started.elapsed() < MAX_PLAYBACK => {}
                _ => state.stop_child(),
            }
        }
        if state.active.is_none() {
            if let Some((text, trace)) = state.queue.pop_front() {
                let generation = state.generation;
                state.synthesizing = true;
                let ok;
                (state, ok) = await_quiet(state, wake, generation, trace.is_some());
                state.synthesizing = false;
                if !ok {
                    continue;
                }
                // Serialize spawn with cancellation so no stale utterance can
                // start after cancel has cleared the queue.
                if let Ok(child) = backend(&text) {
                    if let Some(trace) = trace {
                        trace.mark("player_started", 0);
                    }
                    crate::timing::mark("speech playback process started");
                    state.active = Some((child, Instant::now()));
                }
                continue;
            }
            state = wake.wait(state).unwrap_or_else(|e| e.into_inner());
        } else {
            state = wake
                .wait_timeout(state, POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

fn probe(executable: &str) -> bool {
    let Ok(child) = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let mut state = State::default();
    state.active = Some((child, Instant::now()));
    loop {
        let (child, started) = state.active.as_mut().unwrap();
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < Duration::from_secs(1) => thread::sleep(POLL),
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(start.elapsed() < Duration::from_secs(3), "worker timed out");
            thread::sleep(POLL);
        }
    }
    #[test]
    fn validates_text_without_invoking_backend() {
        let player =
            SpeechPlayer::with_backend(Arc::new(|_| panic!("invalid text played"))).unwrap();
        for text in ["", " \n", "a\0b", &"x".repeat(MAX_TEXT_BYTES + 1)] {
            assert_eq!(player.enqueue(text), Err(SpeechError::InvalidText));
        }
    }
    #[test]
    fn fifo_and_spawn_failure_recovery() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let observed = seen.clone();
        let player = SpeechPlayer::with_backend(Arc::new(move |text| {
            observed.lock().unwrap().push(text.to_owned());
            Err(std::io::Error::other("fake failed command"))
        }))
        .unwrap();
        for text in ["one", "two", "three"] {
            player.enqueue(text).unwrap();
        }
        wait_until(|| seen.lock().unwrap().len() == 3);
        assert_eq!(*seen.lock().unwrap(), ["one", "two", "three"]);
    }
    #[cfg(unix)]
    #[test]
    fn bounded_cancel_reusable_and_drop_reaps_real_fake_child() {
        let player =
            SpeechPlayer::with_backend(Arc::new(|_| Command::new("sleep").arg("30").spawn()))
                .unwrap();
        player.enqueue("active").unwrap();
        wait_until(|| player.shared.0.lock().unwrap().active.is_some());
        let pid = player
            .shared
            .0
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .0
            .id();
        for _ in 0..MAX_QUEUED_UTTERANCES {
            player.enqueue("queued").unwrap();
        }
        assert_eq!(player.enqueue("overflow"), Err(SpeechError::QueueFull));
        player.cancel();
        {
            let state = player.shared.0.lock().unwrap();
            assert!(state.active.is_none());
            assert!(state.queue.is_empty());
        }
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        player.enqueue("reusable").unwrap();
        wait_until(|| player.shared.0.lock().unwrap().active.is_some());
        let pid = player
            .shared
            .0
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .0
            .id();
        drop(player);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
    #[cfg(unix)]
    #[test]
    fn successful_children_advance_fifo() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let observed = seen.clone();
        let player = SpeechPlayer::with_backend(Arc::new(move |text| {
            observed.lock().unwrap().push(text.to_owned());
            Command::new("true").spawn()
        }))
        .unwrap();
        player.enqueue("first").unwrap();
        player.enqueue("second").unwrap();
        wait_until(|| {
            let state = player.shared.0.lock().unwrap();
            seen.lock().unwrap().len() == 2 && state.active.is_none()
        });
        assert_eq!(*seen.lock().unwrap(), ["first", "second"]);
    }
    #[test]
    fn missing_executable_is_unavailable() {
        assert!(!probe("jcode-deliberately-missing-speech-command-71293"));
    }
}
