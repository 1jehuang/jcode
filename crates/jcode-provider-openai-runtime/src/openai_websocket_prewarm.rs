//! Best-effort v2 request prewarming. Only static instructions/tools are sent.
//! A foreground request never waits for warmup: it takes a ready, compatible
//! socket or cancels the speculative work and uses the normal transport path.

use super::*;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::http::Request;

pub(super) const WEBSOCKET_V2_BETA: &str = "responses_websockets=2026-02-06";
const PREWARM_TIMEOUT: Duration = Duration::from_secs(5);
const PREWARM_TTL: Duration = Duration::from_secs(30);

pub(super) fn websocket_request(
    credentials: &CodexCredentials,
    access_token: &str,
) -> Result<Request<()>> {
    let mut request = OpenAIProvider::responses_ws_url(credentials).into_client_request()?;
    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {access_token}"))?,
    );
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    headers.insert("OpenAI-Beta", HeaderValue::from_static(WEBSOCKET_V2_BETA));
    if OpenAIProvider::is_chatgpt_mode(credentials) {
        headers.insert("originator", HeaderValue::from_static(ORIGINATOR));
        if let Some(account_id) = credentials.account_id.as_ref() {
            headers.insert("chatgpt-account-id", HeaderValue::from_str(account_id)?);
        }
    }
    Ok(request)
}

/// The prepared prefix has no conversation items. Compare every remaining
/// request setting, not just model or item counts, before reusing its ID.
fn prewarm_request(request: &Value) -> Value {
    let mut settings: serde_json::Map<String, Value> = request
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "input" | "stream" | "background" | "previous_response_id"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    settings.insert("type".into(), Value::String("response.create".into()));
    settings.insert("input".into(), serde_json::json!([]));
    settings.insert("generate".into(), Value::Bool(false));
    Value::Object(settings)
}

struct PrewarmJob {
    request: Value,
    started_at: Instant,
    ready: oneshot::Receiver<PersistentWsState>,
    task: tokio::task::JoinHandle<()>,
}

pub(super) fn prewarm_identity(credentials: &CodexCredentials) -> (String, Option<String>, String) {
    (
        credentials.access_token.clone(),
        credentials.account_id.clone(),
        OpenAIProvider::responses_ws_url(credentials),
    )
}

impl Drop for PrewarmJob {
    fn drop(&mut self) {
        // Dropping JoinHandle alone would detach a speculative network request.
        self.task.abort();
    }
}

#[derive(Default)]
pub(super) struct PrewarmSlot(StdMutex<Option<PrewarmJob>>);

impl PrewarmSlot {
    #[cfg(test)]
    pub(super) fn is_ready(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|job| !job.ready.is_empty())
    }

    pub(super) fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    pub(super) fn start(
        self: &Arc<Self>,
        credentials: Arc<RwLock<CodexCredentials>>,
        request: &Value,
    ) {
        let request = prewarm_request(request);
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if guard
            .as_ref()
            .is_some_and(|job| job.request == request && job.started_at.elapsed() < PREWARM_TTL)
        {
            return;
        }
        let (tx, ready) = oneshot::channel();
        let started_at = Instant::now();
        let weak = Arc::downgrade(self);
        let payload = request.clone();
        let task = tokio::spawn(async move {
            let model = openai_request_model(&payload);
            let result = tokio::time::timeout(PREWARM_TIMEOUT, async {
                let creds = credentials.read().await;
                // Speculation is cancellable at any instant. Never rotate an
                // OAuth refresh token here: cancellation after server-side
                // rotation could lose the replacement credentials. Leave
                // refresh to the ordinary foreground authentication path.
                anyhow::ensure!(
                    !creds.access_token.is_empty()
                        && !creds.expires_at.is_some_and(|expires| {
                            expires < chrono::Utc::now().timestamp_millis() + 300_000
                        }),
                    "credentials are not ready for prewarm"
                );
                let handshake = websocket_request(&creds, &creds.access_token)?;
                let identity = prewarm_identity(&creds);
                drop(creds);
                let (socket, _) = connect_async(handshake).await?;
                warm_socket(socket, &payload, identity).await
            })
            .await;
            match result {
                Ok(Ok(state)) => {
                    log_openai_stream_lifecycle(
                        jcode_base::logging::LogLevel::Info,
                        "ws_prewarm_ready",
                        vec![
                            ("model", model),
                            ("elapsed_ms", started_at.elapsed().as_millis().to_string()),
                            ("protocol", WEBSOCKET_V2_BETA.to_string()),
                        ],
                    );
                    let _ = tx.send(state);
                }
                _ => {
                    // Warmup is optional. Do not surface its output/errors or
                    // poison the foreground request's transport cooldown.
                    drop(tx);
                    log_openai_stream_lifecycle(
                        jcode_base::logging::LogLevel::Info,
                        "ws_prewarm_unavailable",
                        vec![("model", model)],
                    );
                }
            }
            // Bound idle sockets even if the user never sends the next turn.
            tokio::time::sleep_until((started_at + PREWARM_TTL).into()).await;
            if let Some(slot) = weak.upgrade() {
                let mut guard = slot.0.lock().unwrap_or_else(|e| e.into_inner());
                if guard
                    .as_ref()
                    .is_some_and(|job| job.started_at == started_at)
                {
                    guard.take();
                }
            }
        });
        *guard = Some(PrewarmJob {
            request,
            started_at,
            ready,
            task,
        });
    }

    pub(super) fn take_ready(
        &self,
        request: &Value,
        credentials: &CodexCredentials,
    ) -> Option<PersistentWsState> {
        let mut job = self.0.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        let compatible =
            job.request == prewarm_request(request) && job.started_at.elapsed() < PREWARM_TTL;
        let state = compatible
            .then(|| job.ready.try_recv().ok())
            .flatten()
            .filter(|state| state.identity == prewarm_identity(credentials));
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            if state.is_some() {
                "ws_prewarm_hit"
            } else {
                "ws_prewarm_miss"
            },
            vec![
                ("model", openai_request_model(request)),
                ("compatible", compatible.to_string()),
                ("age_ms", job.started_at.elapsed().as_millis().to_string()),
            ],
        );
        state
    }
}

async fn warm_socket(
    mut socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    request: &Value,
    identity: (String, Option<String>, String),
) -> Result<PersistentWsState> {
    let connected_at = Instant::now();
    socket
        .send(WsMessage::Text(serde_json::to_string(request)?))
        .await?;
    let mut created_id = None;
    while let Some(message) = socket.next().await {
        match message? {
            WsMessage::Text(text) => {
                let event: Value = serde_json::from_str(&text)?;
                match event.get("type").and_then(Value::as_str) {
                    Some("response.created") => {
                        created_id = event["response"]["id"].as_str().map(str::to_owned);
                    }
                    Some("response.completed") => {
                        let response = &event["response"];
                        anyhow::ensure!(
                            response["status"] == "completed",
                            "warmup did not complete"
                        );
                        anyhow::ensure!(
                            response["output"].as_array().is_some_and(Vec::is_empty),
                            "warmup unexpectedly generated output"
                        );
                        let id = response["id"]
                            .as_str()
                            .filter(|id| !id.is_empty())
                            .context("warmup missing response ID")?;
                        anyhow::ensure!(
                            created_id.as_deref() == Some(id),
                            "warmup response ID mismatch"
                        );
                        return Ok(PersistentWsState {
                            ws_stream: socket,
                            identity,
                            last_response_id: id.to_owned(),
                            connected_at,
                            last_activity_at: Instant::now(),
                            last_response_completed_at: Instant::now(),
                            // No generated response items exist yet. The first
                            // continuation must send ALL input, including reasoning.
                            message_count: 0,
                            last_input_item_count: 0,
                        });
                    }
                    Some("response.in_progress") => {}
                    _ => anyhow::bail!("unexpected warmup event"),
                }
            }
            WsMessage::Ping(payload) => socket.send(WsMessage::Pong(payload)).await?,
            WsMessage::Pong(_) => {}
            _ => anyhow::bail!("warmup socket closed or sent a non-text event"),
        }
    }
    anyhow::bail!("warmup ended without completion")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> CodexCredentials {
        CodexCredentials {
            access_token: "test-access".into(),
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        }
    }

    #[test]
    fn v2_handshake_preserves_api_key_and_oauth_authentication() {
        let mut creds = credentials();
        let api = websocket_request(&creds, "test-access").unwrap();
        assert_eq!(api.headers()["OpenAI-Beta"], WEBSOCKET_V2_BETA);
        assert_eq!(api.headers()["Authorization"], "Bearer test-access");
        assert!(!api.headers().contains_key("originator"));
        assert!(!api.headers().contains_key("chatgpt-account-id"));
        creds.refresh_token = "test-refresh".into();
        creds.account_id = Some("test-account".into());
        let oauth = websocket_request(&creds, "test-access").unwrap();
        assert_eq!(oauth.headers()["OpenAI-Beta"], WEBSOCKET_V2_BETA);
        assert_eq!(oauth.headers()["Authorization"], "Bearer test-access");
        assert_eq!(oauth.headers()["originator"], ORIGINATOR);
        assert_eq!(oauth.headers()["chatgpt-account-id"], "test-account");
    }

    #[tokio::test]
    async fn foreground_cancels_matching_unfinished_warmup_without_waiting() {
        let request = serde_json::json!({"model":"gpt-5.6-sol", "input":[]});
        let slot = PrewarmSlot::default();
        let (sender, ready) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _sender = sender;
            std::future::pending::<()>().await;
        });
        let abort = task.abort_handle();
        *slot.0.lock().unwrap() = Some(PrewarmJob {
            request: prewarm_request(&request),
            started_at: Instant::now(),
            ready,
            task,
        });
        assert!(slot.take_ready(&request, &credentials()).is_none());
        tokio::task::yield_now().await;
        assert!(abort.is_finished(), "cancelled warmup must not detach");
        assert!(slot.0.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_credentials_are_not_refreshed_by_speculation() {
        let mut creds = credentials();
        creds.refresh_token = "must-never-be-sent-to-refresh-endpoint".into();
        creds.expires_at = Some(0);
        let credentials = Arc::new(RwLock::new(creds));
        let slot = Arc::new(PrewarmSlot::default());
        slot.start(
            Arc::clone(&credentials),
            &serde_json::json!({"model":"gpt-5.6-sol"}),
        );
        tokio::task::yield_now().await;
        let mut job = slot.0.lock().unwrap().take().unwrap();
        assert!(
            matches!(
                job.ready.try_recv(),
                Err(oneshot::error::TryRecvError::Closed)
            ),
            "expired credentials should be rejected immediately, without networking"
        );
        assert_eq!(credentials.read().await.access_token, "test-access");
    }

    #[tokio::test]
    async fn expired_ready_warmup_is_closed_instead_of_adopted() {
        let (mut state, server) = crate::tests::test_persistent_ws_state().await;
        state.identity = prewarm_identity(&credentials());
        let request = serde_json::json!({"model":"gpt-5.6-sol", "input":[]});
        let slot = PrewarmSlot::default();
        let (sender, ready) = oneshot::channel();
        assert!(sender.send(state).is_ok());
        *slot.0.lock().unwrap() = Some(PrewarmJob {
            request: prewarm_request(&request),
            started_at: Instant::now() - PREWARM_TTL,
            ready,
            task: tokio::spawn(std::future::pending()),
        });
        assert!(slot.take_ready(&request, &credentials()).is_none());
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("expired socket should close")
            .expect("socket server");
    }

    #[test]
    fn warmup_only_prepares_settings_and_keeps_original_request_unchanged() {
        let request = serde_json::json!({
            "model": "gpt-5.6-sol", "instructions": "static context",
            "tools": [{"type":"function", "name":"test"}],
            "input": [{"type":"function_call_output", "call_id":"pending", "output":"secret"}],
            "stream": true, "background": false, "store": false,
            "previous_response_id": "old_chain", "reasoning": {"effort":"low"}
        });
        let warm = prewarm_request(&request);
        assert_eq!(warm["type"], "response.create");
        assert_eq!(warm["generate"], false);
        assert_eq!(warm["input"], serde_json::json!([]));
        assert_eq!(warm["store"], false);
        for key in ["stream", "background", "previous_response_id"] {
            assert!(warm.get(key).is_none(), "unexpected warmup field: {key}");
        }
        for key in ["instructions", "tools", "reasoning", "model"] {
            assert_eq!(warm[key], request[key]);
        }
        assert_eq!(request["input"][0]["output"], "secret");
        assert!(request.get("generate").is_none());
    }

    #[test]
    fn warmup_compatibility_compares_all_settings_but_not_conversation_input() {
        let request = serde_json::json!({
            "model":"gpt-5.6-sol", "instructions":"static", "input":[], "tools":[],
            "store":false, "reasoning":{"effort":"low"}, "service_tier":"auto",
            "max_output_tokens":100, "prompt_cache_key":"a", "prompt_cache_retention":"24h",
            "parallel_tool_calls":false, "tool_choice":"auto", "include":["reasoning.encrypted_content"],
            "context_management":[{"type":"compaction", "compact_threshold":1000}]
        });
        let mut next = request.clone();
        next["input"] = serde_json::json!([{"role":"user", "content":"new message"}]);
        assert_eq!(prewarm_request(&request), prewarm_request(&next));
        for key in request
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| key.as_str() != "input")
        {
            let mut changed = next.clone();
            changed.as_object_mut().unwrap().remove(key);
            assert_ne!(
                prewarm_request(&request),
                prewarm_request(&changed),
                "must compare {key}"
            );
        }
    }

    /// Exercises the production provider against the configured OpenAI account,
    /// independent of the running Jcode daemon. Uses a few short generations.
    /// Timings are observations, not a statistically valid speedup benchmark.
    #[tokio::test]
    #[ignore = "uses configured OpenAI credentials and real model requests"]
    async fn live_openai_v2_prewarm_and_continuation() -> Result<()> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let creds = jcode_base::auth::codex::load_credentials()?;
        let model =
            std::env::var("JCODE_OPENAI_LIVE_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string());
        let system = "Reply to each message with exactly OK. Do not use tools.";
        let messages = vec![ChatMessage::user("Connection test.")];
        let mut times = Vec::new();
        for warmed in [false, true] {
            let provider = OpenAIProvider::new(creds.clone());
            provider.set_model(&model)?;
            provider.set_transport("websocket")?;
            provider.set_reasoning_effort("low")?;
            if warmed {
                let started = Instant::now();
                provider.prewarm(&[], system).await;
                tokio::time::timeout(PREWARM_TIMEOUT + Duration::from_secs(1), async {
                    while !provider.prewarm.is_ready() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .context("live warmup never became ready")?;
                eprintln!("v2 warmup ready in {}ms", started.elapsed().as_millis());
            }
            let start = Instant::now();
            let mut stream = provider.complete(&messages, &[], system, None).await?;
            let mut text = String::new();
            let mut connection = String::new();
            let mut first_text_ms = None;
            while let Some(event) = stream.next().await {
                match event? {
                    StreamEvent::TextDelta(delta) => {
                        first_text_ms.get_or_insert_with(|| start.elapsed().as_millis());
                        text.push_str(&delta);
                    }
                    StreamEvent::ConnectionType { connection: value } => connection = value,
                    _ => {}
                }
            }
            anyhow::ensure!(text.trim() == "OK", "unexpected live response");
            anyhow::ensure!(
                connection.starts_with("websocket"),
                "live request fell back to {connection}"
            );
            if warmed {
                anyhow::ensure!(
                    connection.contains("persistent-reuse"),
                    "warm socket was not consumed: {connection}"
                );
                let mut continuation = messages.clone();
                continuation.push(ChatMessage::assistant_text(&text));
                continuation.push(ChatMessage::user("Continuation test."));
                let mut stream = provider.complete(&continuation, &[], system, None).await?;
                let mut continued = String::new();
                while let Some(event) = stream.next().await {
                    if let StreamEvent::TextDelta(delta) = event? {
                        continued.push_str(&delta);
                    }
                }
                anyhow::ensure!(continued.trim() == "OK", "live continuation failed");
                anyhow::ensure!(
                    provider
                        .persistent_ws
                        .lock()
                        .await
                        .as_ref()
                        .is_some_and(|state| state.message_count == 2),
                    "continuation did not retain the warmed response chain"
                );
            }
            times.push((warmed, first_text_ms, connection));
        }
        eprintln!("v2 observed foreground time-to-first-text (one sample each): {times:?}");
        Ok(())
    }
}
