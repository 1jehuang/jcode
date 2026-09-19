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

/// Full aggregate revisions, not deltas. Finished is emitted exactly once by run().
#[derive(Clone, PartialEq, Eq)]
pub enum NariEvent {
    Started,
    Transcript(String),
    Finished(Result<String, VoiceError>),
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
            send(&mut socket, json!({"type":"session.configure", "session":{"model":"qwen3-asr-fast", "turn_detection":null, "language":"en"}})).await?;
            loop {
                let event = receive(&mut socket).await?;
                match event["type"].as_str() {
                    Some("session.configured") => return Ok(socket),
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

    // Supervise provider failures while native permission/setup is in progress.
    #[cfg(feature = "voice-capture")]
    pub(super) async fn wait_for_error(&mut self) -> VoiceError {
        loop {
            match receive(&mut self.socket).await {
                Ok(event) if event["type"] == "error" => return provider_error(&event),
                Ok(_) => {}
                Err(error) => return error,
            }
        }
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
        self.items
            .iter()
            .map(|(_, text)| text.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
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
