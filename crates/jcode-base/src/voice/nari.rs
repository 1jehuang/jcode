//! Native Nari streaming. No audio, transcript, credential, or provider body is logged.
use super::{MAX_RECORDING_DURATION, MAX_RESPONSE_BYTES, VoiceError};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::mpsc,
    time::{Instant, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

pub(super) const URL: &str = "wss://api.narilabs.com/v1/realtime?intent=transcription";
const END: &str = "jcode_end";
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const FINAL_TIMEOUT: Duration = Duration::from_secs(20);
pub const NARI_PCM_CHUNK_SAMPLES: usize = 6400;
/// Published `qwen3-asr-fast` list price per input audio hour.
/// Source: <https://docs.narilabs.com/models-and-pricing> (checked 2026-09-22).
pub const NARI_USD_PER_AUDIO_HOUR: f64 = 0.12;

/// Estimated Nari transcription cost for the given streamed audio duration.
pub fn estimated_transcription_usd(audio: Duration) -> f64 {
    audio.as_secs_f64() / 3600.0 * NARI_USD_PER_AUDIO_HOUR
}

/// Full aggregate revisions, not deltas. Finished is emitted exactly once by run().
#[derive(Clone, PartialEq, Eq)]
pub enum NariEvent {
    Started,
    Transcript(String),
    Finished(Result<String, VoiceError>),
}

/// Describes the assistant name in context. Qwen3-ASR uses the prompt as
/// biasing context, and a descriptive sentence with example addresses beats a
/// bare term list: on noisy synthetic speech it doubled exact "Jev" (12/18 vs
/// 6/18, repeatable), recovering "Jen", "Gem", "Kim" and dropped names.
const NAME_CONTEXT: &str = "The user often addresses Jev, a voice assistant. \
Jev is spelled J-E-V and sounds like Jeff. Write it as Jev. \
Examples: \"Hey Jev, open settings.\" \"Okay Jev.\" \"Thanks Jev.\" \"Ask Jev.\" \
Other names: ";

/// Names Qwen3-ASR otherwise mishears. Keep proper casing: it is copied as-is.
/// "Jev" is covered by `NAME_CONTEXT`, so it is deduplicated from this list.
const BUILTIN_VOCABULARY: &[&str] = &[
    "Jev",
    "Jcode",
    "Jcode Desktop",
    "Handterm",
    "Nari",
    "TypeSafe",
    "GPUI",
    "Wayland",
    "niri",
    "Copilot",
    "swarm",
    "hot reload",
    "Claude",
    "Codex",
    "OpenAI",
    "Anthropic",
];
/// Mishearings the recognition prompt cannot fix, because the audio is
/// genuinely ambiguous ("Jev" is pronounced like "Jeff"). Applied to every
/// transcript revision as whole-word, case-insensitive replacements.
const BUILTIN_CORRECTIONS: &[(&str, &str)] = &[
    (r"jeff", "Jev"),
    (r"j[\s.-]?code", "Jcode"),
    (r"jay[\s-]?code", "Jcode"),
];

fn corrections() -> &'static [(regex::Regex, &'static str)] {
    static CORRECTIONS: std::sync::OnceLock<Vec<(regex::Regex, &'static str)>> =
        std::sync::OnceLock::new();
    CORRECTIONS.get_or_init(|| {
        BUILTIN_CORRECTIONS
            .iter()
            .map(|(pattern, fixed)| {
                let pattern = format!(r"(?i)\b{pattern}\b");
                (
                    regex::Regex::new(&pattern).expect("valid correction"),
                    *fixed,
                )
            })
            .collect()
    })
}

/// Apply product-name corrections to a transcript.
pub fn correct_transcript(text: &str) -> String {
    corrections()
        .iter()
        .fold(text.to_owned(), |text, (pattern, fixed)| {
            pattern.replace_all(&text, *fixed).into_owned()
        })
}

/// Conservative bound on the recognition context sent per session.
const MAX_PROMPT_CHARS: usize = 1000;

/// Recognition context: built-in names plus `[dictation] vocabulary`,
/// deduplicated case-insensitively and bounded without splitting a term.
pub fn recognition_prompt() -> String {
    build_prompt(&crate::config::config().dictation.vocabulary)
}

fn build_prompt(extra: &[String]) -> String {
    let mut seen = HashSet::from(["jev".to_string()]);
    let mut prompt = String::new();
    let terms = BUILTIN_VOCABULARY
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str));
    for term in terms {
        if term.chars().any(char::is_control) {
            continue;
        }
        let term = term.split_whitespace().collect::<Vec<_>>().join(" ");
        let term = term.trim_matches(',').trim();
        if term.is_empty() {
            continue;
        }
        if !seen.insert(term.to_lowercase()) {
            continue;
        }
        let sep = if prompt.is_empty() { "" } else { ", " };
        let used = NAME_CONTEXT.chars().count() + prompt.chars().count() + 1;
        if used + sep.len() + term.chars().count() > MAX_PROMPT_CHARS {
            break;
        }
        prompt.push_str(sep);
        prompt.push_str(term);
    }
    format!("{NAME_CONTEXT}{prompt}.")
}

pub fn nari_api_key() -> Option<String> {
    crate::provider_catalog::load_api_key_from_env_or_config("NARI_API_KEY", "nari.env")
        .filter(|key| !key.trim().is_empty())
}

/// Bounded mono 16 kHz PCM16 source. Drop all senders to commit and finalize.
/// Chunks must contain 1..=6400 samples. Backpressure is intentional.
pub fn nari_pcm_channel() -> (mpsc::Sender<Vec<i16>>, mpsc::Receiver<Vec<i16>>) {
    mpsc::channel(16)
}

/// Microphone-owned channel. Holds 100 ms chunks for longer than the setup
/// timeout so audio captured during the handshake is buffered, never dropped.
#[cfg(feature = "voice-capture")]
pub(super) fn capture_pcm_channel() -> (mpsc::Sender<Vec<i16>>, mpsc::Receiver<Vec<i16>>) {
    mpsc::channel((IO_TIMEOUT.as_secs() as usize + 5) * 10)
}

pub(super) async fn cancelled(cancel: &AtomicBool) {
    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub struct NariSession {
    socket: Socket,
    cancel: Arc<AtomicBool>,
}

impl NariSession {
    /// Resolves only after the server acknowledges session.configure. No microphone access.
    pub async fn connect(key: &str, cancel: Arc<AtomicBool>) -> Result<Self, VoiceError> {
        Self::connect_to(URL, key, cancel).await
    }
    pub(super) async fn connect_to(
        url: &str,
        key: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, VoiceError> {
        Self::connect_with_prompt(url, key, &recognition_prompt(), cancel).await
    }
    pub(super) async fn connect_with_prompt(
        url: &str,
        key: &str,
        prompt: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, VoiceError> {
        if cancel.load(Ordering::SeqCst) {
            return Err(VoiceError::Cancelled);
        }
        if key.is_empty() || key.len() > 1024 || !key.bytes().all(|b| (33..=126).contains(&b)) {
            return Err(VoiceError::NariNotConfigured);
        }
        let setup = async {
            // Standalone library callers may not have initialized TLS. Preserve
            // any installed provider, otherwise use the same one as Jcode main.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let mut request = url.into_client_request().map_err(|_| VoiceError::Network)?;
            let mut auth = format!("Bearer {key}")
                .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
                .map_err(|_| VoiceError::NariNotConfigured)?;
            auth.set_sensitive(true);
            request.headers_mut().insert("Authorization", auth);
            let config = WebSocketConfig {
                max_message_size: Some(MAX_RESPONSE_BYTES),
                max_frame_size: Some(MAX_RESPONSE_BYTES),
                ..Default::default()
            };
            let (mut socket, _) =
                tokio_tungstenite::connect_async_with_config(request, Some(config), false)
                    .await
                    .map_err(handshake_error)?;
            super::timing::mark("nari websocket connected");
            send(&mut socket, json!({"type":"session.configure", "session":{"model":"qwen3-asr-fast", "turn_detection":null, "language":"en", "prompt":prompt}})).await?;
            loop {
                let event = receive(&mut socket).await?;
                match event["type"].as_str() {
                    Some("session.configured") => {
                        super::timing::mark("nari session configured");
                        return Ok(socket);
                    }
                    Some("error") => return Err(provider_error(&event)),
                    _ => {}
                }
            }
        };
        let socket = tokio::select! {
            biased;
            _ = cancelled(&cancel) => return Err(VoiceError::Cancelled),
            result = timeout(IO_TIMEOUT, setup) => result.map_err(|_| VoiceError::Timeout)??,
        };
        Ok(Self { socket, cancel })
    }

    /// Stream bounded PCM from any source. Callback must be fast and nonblocking.
    /// EOF sends an explicit final commit, then waits for its acknowledgement AND
    /// every pending utterance, including duration auto-commits. Dropping this
    /// future closes the socket. Cancellation interrupts setup, reads and writes.
    pub async fn run(
        self,
        pcm: mpsc::Receiver<Vec<i16>>,
        mut event: impl FnMut(NariEvent),
    ) -> Result<String, VoiceError> {
        let cancel = self.cancel.clone();
        event(NariEvent::Started);
        let result = tokio::select! {
            biased;
            _ = cancelled(&cancel) => Err(VoiceError::Cancelled),
            result = self.run_inner(pcm, &mut event) => result,
        };
        event(NariEvent::Finished(result.clone()));
        result
    }
    async fn run_inner(
        self,
        mut pcm: mpsc::Receiver<Vec<i16>>,
        event: &mut impl FnMut(NariEvent),
    ) -> Result<String, VoiceError> {
        let (mut sink, mut source) = self.socket.split();
        let stopping = Arc::new(AtomicBool::new(false));
        let sender_stopping = stopping.clone();
        let sender = async move {
            let mut samples = 0usize;
            while let Some(chunk) = pcm.recv().await {
                if samples == 0 {
                    super::timing::mark("first audio chunk sent to nari");
                }
                if chunk.is_empty() || chunk.len() > NARI_PCM_CHUNK_SAMPLES {
                    return Err(VoiceError::InvalidAudio);
                }
                samples += chunk.len();
                if samples > 16000 * MAX_RECORDING_DURATION.as_secs() as usize {
                    return Err(VoiceError::InvalidAudio);
                }
                let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                send(
                    &mut sink,
                    json!({"type":"input_audio_buffer.append", "audio":STANDARD.encode(bytes)}),
                )
                .await?;
            }
            sender_stopping.store(true, Ordering::SeqCst);
            send(
                &mut sink,
                json!({"type":"input_audio_buffer.commit", "event_id":END}),
            )
            .await
        };
        tokio::pin!(sender);
        let mut sent = false;
        let mut first_text = false;
        let mut state = TranscriptState::default();
        // Capture owns its normal five-minute stop. Allow queued PCM and filter
        // tail to drain rather than racing that stop with a network timeout.
        let mut deadline = Instant::now() + MAX_RECORDING_DURATION + IO_TIMEOUT;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Err(VoiceError::Timeout),
                result = &mut sender, if !sent => {
                    result?;
                    sent = true;
                    deadline = Instant::now() + FINAL_TIMEOUT;
                }
                incoming = receive(&mut source) => {
                    let incoming = incoming?;
                    if state.apply(&incoming, stopping.load(Ordering::SeqCst))? {
                        if !first_text { first_text = true; super::timing::mark("first transcript revision"); }
                        event(NariEvent::Transcript(state.text()));
                    }
                    if stopping.load(Ordering::SeqCst) && state.end_ack && state.pending.is_empty() { return Ok(state.text()); }
                }
            }
        }
    }
}
fn handshake_error(error: tokio_tungstenite::tungstenite::Error) -> VoiceError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        match response.status().as_u16() {
            401 | 403 => VoiceError::NariNotConfigured,
            402 => VoiceError::NariCreditsExhausted,
            status => VoiceError::Http(status),
        }
    } else {
        VoiceError::Network
    }
}

async fn send<S>(socket: &mut S, value: Value) -> Result<(), VoiceError>
where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    timeout(IO_TIMEOUT, socket.send(Message::Text(value.to_string())))
        .await
        .map_err(|_| VoiceError::Timeout)?
        .map_err(|_| VoiceError::Network)
}
async fn receive<S>(socket: &mut S) -> Result<Value, VoiceError>
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: Value =
                    serde_json::from_str(&text).map_err(|_| VoiceError::InvalidResponse)?;
                if !value.is_object() {
                    return Err(VoiceError::InvalidResponse);
                }
                return Ok(value);
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                // Tungstenite queues automatic pong responses on read.
            }
            _ => return Err(VoiceError::Network),
        }
    }
}
fn provider_error(event: &Value) -> VoiceError {
    match event["error"]["code"].as_str() {
        Some("UNAUTHORIZED" | "INVALID_API_KEY") => VoiceError::NariNotConfigured,
        Some("INSUFFICIENT_CREDITS") => VoiceError::NariCreditsExhausted,
        Some("RATE_LIMITED") => VoiceError::Http(429),
        _ => VoiceError::NariRejected,
    }
}
#[derive(Default)]
struct TranscriptState {
    items: Vec<(String, String)>,
    pending: HashSet<String>,
    completed: HashSet<String>,
    end_ack: bool,
}
impl TranscriptState {
    fn text(&self) -> String {
        correct_transcript(
            &self
                .items
                .iter()
                .map(|(_, text)| text.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
    fn apply(&mut self, event: &Value, stopping: bool) -> Result<bool, VoiceError> {
        let kind = event["type"].as_str().ok_or(VoiceError::InvalidResponse)?;
        if kind == "error" {
            return Err(provider_error(event));
        }
        let id = event
            .get("item_id")
            .map_or(Some("default"), Value::as_str)
            .ok_or(VoiceError::InvalidResponse)?;
        if id.len() > 256 {
            return Err(VoiceError::InvalidResponse);
        }
        let transcript = matches!(kind, "transcript.partial" | "transcript.completed");
        if kind == "input_audio_buffer.committed" || transcript {
            if !self.items.iter().any(|(item, _)| item == id) {
                if self.items.len() >= 1024 {
                    return Err(VoiceError::ResponseTooLarge);
                }
                self.items.push((id.to_owned(), String::new()));
            }
            if !self.completed.contains(id) {
                self.pending.insert(id.to_owned());
            }
        }
        if transcript {
            let text = event["transcript"]
                .as_str()
                .ok_or(VoiceError::InvalidResponse)?;
            let total: usize = self
                .items
                .iter()
                .filter(|(item, _)| item != id)
                .map(|(_, t)| t.len() + 1)
                .sum();
            if total + text.len() > MAX_RESPONSE_BYTES {
                return Err(VoiceError::ResponseTooLarge);
            }
            // Late partials must not overwrite completed text.
            if kind == "transcript.completed" || !self.completed.contains(id) {
                self.items
                    .iter_mut()
                    .find(|(item, _)| item == id)
                    .unwrap()
                    .1 = text.to_owned();
            }
            if kind == "transcript.completed" {
                self.pending.remove(id);
                self.completed.insert(id.to_owned());
            }
        }
        if matches!(
            kind,
            "input_audio_buffer.committed" | "input_audio_buffer.commit_empty"
        ) && event["client_event_id"] == END
        {
            if !stopping {
                return Err(VoiceError::InvalidResponse);
            }
            self.end_ack = true;
        }
        Ok(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn product_name_mishearings_are_corrected() {
        assert_eq!(
            correct_transcript("Hey, Jeff. Open the JCode desktop and ask jeff's route."),
            "Hey, Jev. Open the Jcode desktop and ask Jev's route."
        );
        assert_eq!(
            correct_transcript("J code, j-code, Jay code"),
            "Jcode, Jcode, Jcode"
        );
        // Whole words only.
        assert_eq!(correct_transcript("Jefferson jcoder"), "Jefferson jcoder");
        assert_eq!(correct_transcript("Jev and Jcode"), "Jev and Jcode");
    }

    #[test]
    fn transcription_cost_uses_published_hourly_rate() {
        assert_eq!(estimated_transcription_usd(Duration::from_secs(3600)), 0.12);
        assert!((estimated_transcription_usd(Duration::from_secs(30)) - 0.001).abs() < 1e-12);
    }

    #[test]
    fn prompt_merges_user_terms_dedupes_and_stays_bounded() {
        let prompt = build_prompt(&[
            "  Alice   Zhang ".into(),
            "jcode".into(),
            "".into(),
            "bad\nterm".into(),
            ",Kubernetes,".into(),
        ]);
        assert!(prompt.starts_with(NAME_CONTEXT));
        assert!(prompt.contains("sounds like Jeff. Write it as Jev."));
        let terms = prompt.strip_prefix(NAME_CONTEXT).unwrap();
        assert!(terms.starts_with("Jcode, Jcode Desktop, Handterm"));
        assert!(terms.ends_with(", Alice Zhang, Kubernetes."));
        assert!(!terms.contains("Jev"), "Jev lives in the context: {terms}");
        assert_eq!(terms.matches("code").count(), 2, "jcode deduped: {prompt}");
        assert!(!prompt.contains("bad"));
        let long = build_prompt(&(0..500).map(|i| format!("term{i}")).collect::<Vec<_>>());
        assert!(long.chars().count() <= MAX_PROMPT_CHARS);
        assert!(
            long.strip_prefix(NAME_CONTEXT)
                .unwrap()
                .trim_end_matches('.')
                .split(", ")
                .all(|t| t.starts_with("term") || BUILTIN_VOCABULARY.contains(&t))
        );
    }
    async fn server(events: Vec<Value>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let config: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(config["type"], "session.configure");
            assert_eq!(config["session"]["model"], "qwen3-asr-fast");
            assert!(
                config["session"]["prompt"]
                    .as_str()
                    .unwrap()
                    .starts_with(NAME_CONTEXT)
            );
            ws.send(Message::Text(
                json!({"type":"session.configured"}).to_string(),
            ))
            .await
            .unwrap();
            loop {
                let message = ws.next().await.unwrap().unwrap();
                let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                if value["type"] == "input_audio_buffer.commit" {
                    assert_eq!(value["event_id"], END);
                    break;
                }
                assert_eq!(value["type"], "input_audio_buffer.append");
                assert_eq!(
                    STANDARD.decode(value["audio"].as_str().unwrap()).unwrap(),
                    [1, 0, 255, 255]
                );
            }
            for event in events {
                ws.send(Message::Text(event.to_string())).await.unwrap();
            }
        });
        (url, task)
    }
    #[tokio::test]
    async fn final_ack_waits_for_all_auto_commits_and_aggregates_revisions() {
        let events = vec![
            json!({"type":"input_audio_buffer.committed","item_id":"a","commit_reason":"duration"}),
            json!({"type":"transcript.partial","item_id":"a","transcript":"Hel"}),
            json!({"type":"input_audio_buffer.committed","item_id":"b","client_event_id":END}),
            json!({"type":"transcript.completed","item_id":"b","transcript":"world"}),
            json!({"type":"transcript.completed","item_id":"a","transcript":"Hello"}),
        ];
        let (url, task) = server(events).await;
        let session = NariSession::connect_to(&url, "test", Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let (tx, rx) = nari_pcm_channel();
        tx.send(vec![1, -1]).await.unwrap();
        drop(tx);
        let mut updates = Vec::new();
        let result = session.run(rx, |e| updates.push(e)).await.unwrap();
        assert_eq!(result, "Hello world");
        assert!(matches!(updates.first(), Some(NariEvent::Started)));
        assert!(matches!(updates.last(),Some(NariEvent::Finished(Ok(s))) if s=="Hello world"));
        task.await.unwrap();
    }
    #[tokio::test]
    async fn empty_final_commit_waits_for_pending_duration_item() {
        let (url, task) = server(vec![
            json!({"type":"input_audio_buffer.committed","item_id":"a"}),
            json!({"type":"input_audio_buffer.commit_empty","client_event_id":END}),
            json!({"type":"transcript.completed","item_id":"a","transcript":"kept"}),
        ])
        .await;
        let session = NariSession::connect_to(&url, "test", Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let (tx, rx) = nari_pcm_channel();
        drop(tx);
        assert_eq!(session.run(rx, |_| {}).await.unwrap(), "kept");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn empty_recording_can_finalize() {
        let (url, task) = server(vec![
            json!({"type":"input_audio_buffer.commit_empty","client_event_id":END}),
        ])
        .await;
        let session = NariSession::connect_to(&url, "test", Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let (tx, rx) = nari_pcm_channel();
        drop(tx);
        assert_eq!(session.run(rx, |_| {}).await.unwrap(), "");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn cancellation_interrupts_handshake_without_credentials_or_audio() {
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            NariSession::connect("", cancel).await,
            Err(VoiceError::Cancelled)
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            c.store(true, Ordering::SeqCst);
        });
        assert!(matches!(
            NariSession::connect_to(
                &format!("ws://{}", listener.local_addr().unwrap()),
                "test",
                cancel
            )
            .await,
            Err(VoiceError::Cancelled)
        ));
        task.await.unwrap();
    }
    #[test]
    fn completed_before_committed_and_late_partial_are_safe() {
        let mut s = TranscriptState::default();
        s.apply(
            &json!({"type":"transcript.completed","item_id":"a","transcript":"final"}),
            false,
        )
        .unwrap();
        s.apply(
            &json!({"type":"input_audio_buffer.committed","item_id":"a"}),
            false,
        )
        .unwrap();
        s.apply(
            &json!({"type":"transcript.partial","item_id":"a","transcript":"stale"}),
            false,
        )
        .unwrap();
        assert!(s.pending.is_empty());
        assert_eq!(s.text(), "final");
        assert_eq!(
            s.apply(
                &json!({"type":"transcript.partial","item_id":42,"transcript":"secret"}),
                false
            ),
            Err(VoiceError::InvalidResponse)
        );
        assert_eq!(
            s.apply(&json!({"type":"error","error":{"message":"secret"}}), false),
            Err(VoiceError::NariRejected)
        );
        assert!(!VoiceError::NariRejected.to_string().contains("secret"));
    }
    #[test]
    fn handshake_statuses_are_sanitized() {
        use tokio_tungstenite::tungstenite::{Error, http::Response};
        for (status, expected) in [
            (401, VoiceError::NariNotConfigured),
            (403, VoiceError::NariNotConfigured),
            (402, VoiceError::NariCreditsExhausted),
            (429, VoiceError::Http(429)),
        ] {
            let response = Response::builder()
                .status(status)
                .body(Some(b"secret provider body".to_vec()))
                .unwrap();
            assert_eq!(handshake_error(Error::Http(response)), expected);
        }
    }
    #[tokio::test]
    async fn wrong_final_ack_cannot_return_success() {
        let (url, task) = server(vec![
            json!({"type":"transcript.completed","item_id":"a","transcript":"kept"}),
            json!({"type":"input_audio_buffer.commit_empty","client_event_id":"wrong"}),
        ])
        .await;
        let session = NariSession::connect_to(&url, "test", Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let (tx, rx) = nari_pcm_channel();
        drop(tx);
        assert_eq!(session.run(rx, |_| {}).await, Err(VoiceError::Network));
        task.await.unwrap();
    }
    #[test]
    fn full_partial_revisions_can_replace_non_prefix_text() {
        let mut s = TranscriptState::default();
        s.apply(
            &json!({"type":"transcript.partial","item_id":"a","transcript":"I scream"}),
            false,
        )
        .unwrap();
        s.apply(
            &json!({"type":"transcript.partial","item_id":"a","transcript":"ice cream"}),
            false,
        )
        .unwrap();
        assert_eq!(s.text(), "ice cream");
    }
}
