use super::openai_stream_cancellation::{after_authentication, while_consumer_open};
use super::openai_stream_diagnostics::{
    StreamProgress, add_cache_comparison, reqwest_header_fields, tungstenite_header_fields,
};
use super::*;

#[path = "openai_ws_cancel_recovery.rs"]
mod openai_ws_cancel_recovery;
use openai_ws_cancel_recovery::{RecoverableAttempt, RecoveryPhase, has_unanswered_tool_calls};

/// Usage of the account these credentials authenticate as, plus its label for
/// logs. Unbound sessions keep the credentials they loaded, so the global
/// active login can be a different account with different quota.
pub(super) fn serving_account_usage(
    credentials: &CodexCredentials,
) -> (Option<String>, jcode_base::usage::OpenAIUsageData) {
    let label = jcode_base::auth::codex::list_accounts()
        .ok()
        .and_then(|accounts| {
            jcode_base::auth::codex::account_label_for_credentials(&accounts, credentials)
        });
    let usage = match label.as_deref() {
        Some(label) => jcode_base::usage::get_openai_usage_for_label_sync(label),
        // Not a saved login (legacy single-account file): the active usage is it.
        None => jcode_base::usage::get_openai_usage_sync(),
    };
    (label, usage)
}

/// `account=<label> <usage fields>` for limit diagnostics.
pub(super) fn serving_account_diag(credentials: &CodexCredentials) -> (String, bool) {
    let (label, usage) = serving_account_usage(credentials);
    (
        format!(
            "account={} {}",
            label.as_deref().unwrap_or("unknown"),
            usage.diagnostic_fields()
        ),
        usage.exhausted(),
    )
}

// Bind metadata to the immutable request, not the provider's concurrently mutable
// preference. Both primary parser events and its pending queue pass here.
fn bind_requested_service_tier(event: &mut StreamEvent, request_tier: &Option<String>) {
    if let StreamEvent::ServiceTier { requested, .. } = event {
        *requested = request_tier.clone();
    }
}

#[test]
fn service_tier_metadata_uses_serialized_request_not_later_preference() {
    let request = serde_json::json!({"service_tier": "priority"});
    let requested = request
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut event = StreamEvent::ServiceTier {
        tier: "default".into(),
        requested: None,
    };
    bind_requested_service_tier(&mut event, &requested);
    assert!(
        matches!(event, StreamEvent::ServiceTier { requested: Some(ref tier), .. } if tier == "priority")
    );
    // An omitted request field must stay omitted, even if a preference has
    // changed to Ultrafast while the network request was in flight.
    bind_requested_service_tier(&mut event, &None);
    assert!(matches!(
        event,
        StreamEvent::ServiceTier {
            requested: None,
            ..
        }
    ));
}

#[path = "openai_usage_recording.rs"]
mod openai_usage_recording;
use openai_usage_recording::OAuthUsageRecorder;

#[path = "openai_rate_limit_format.rs"]
mod openai_rate_limit_format;
use self::openai_rate_limit_format::format_rate_limit_error;
#[path = "openai_stream_timeout.rs"]
mod openai_stream_timeout;
pub(super) use self::openai_stream_timeout::reasoning_payload;
use self::openai_stream_timeout::{
    effective_https_idle_timeout, effective_ws_completion_timeout_secs, send_websocket_frame,
};

pub(super) async fn openai_access_token(
    account_label: Option<String>,
    credentials: &Arc<RwLock<CodexCredentials>>,
) -> anyhow::Result<String> {
    if let Some(label) = account_label.as_deref() {
        let is_oauth = {
            let tokens = credentials.read().await;
            !tokens.refresh_token.is_empty() || tokens.id_token.is_some()
        };
        if is_oauth {
            // Cloud handoff/login can replace the file while this daemon keeps
            // its provider alive. Never send or refresh the old cached copy.
            let mut tokens = credentials.write().await;
            *tokens = jcode_base::auth::codex::load_credentials_for_account(label)?;
        }
    }
    let (access_token, refresh_token, needs_refresh, is_oauth) = {
        let tokens = credentials.read().await;
        if tokens.access_token.is_empty() {
            anyhow::bail!("OpenAI access token is empty");
        }

        let should_refresh = if let Some(expires_at) = tokens.expires_at {
            expires_at < chrono::Utc::now().timestamp_millis() + 300_000
                && !tokens.refresh_token.is_empty()
        } else {
            false
        };

        (
            tokens.access_token.clone(),
            tokens.refresh_token.clone(),
            should_refresh,
            !tokens.refresh_token.is_empty() || tokens.id_token.is_some(),
        )
    };

    if is_oauth && let Some(label) = account_label.as_deref() {
        // A deleted explicit login must not continue using its cached bearer.
        jcode_base::auth::codex::load_credentials_for_account(label)?;
    }

    if !needs_refresh {
        return Ok(access_token);
    }

    if refresh_token.is_empty() {
        return Ok(access_token);
    }

    force_refresh_openai_token(credentials, &refresh_token, account_label).await
}

/// Unconditionally refresh the OpenAI access token using the stored refresh
/// token, persisting the rotated credentials in place. Used when the server
/// rejects the current access token (401/403) even though it had not yet hit
/// its local expiry window.
pub(super) async fn force_refresh_openai_token(
    credentials: &Arc<RwLock<CodexCredentials>>,
    refresh_token: &str,
    account_label: Option<String>,
) -> anyhow::Result<String> {
    // Rotation, persistence, and the in-memory commit form one transaction.
    // It runs as its own task so a cancelled caller (dropped stream, aborted
    // turn) cannot strand rotated tokens on disk while this provider keeps the
    // consumed refresh token in memory. The network wait inside is bounded by
    // `oauth::OPENAI_REFRESH_TIMEOUT`.
    let refresh_token = refresh_token.to_string();
    refresh_and_commit_openai_tokens(credentials, async move {
        match account_label {
            Some(label) => oauth::refresh_openai_tokens_for_account(&refresh_token, &label).await,
            None => oauth::refresh_openai_tokens(&refresh_token).await,
        }
    })
    .await
}

/// Run `refresh` (rotation plus persistence) and the in-memory commit as one
/// task that survives cancellation of the caller.
pub(super) async fn refresh_and_commit_openai_tokens(
    credentials: &Arc<RwLock<CodexCredentials>>,
    refresh: impl std::future::Future<Output = anyhow::Result<oauth::OAuthTokens>> + Send + 'static,
) -> anyhow::Result<String> {
    let credentials = Arc::clone(credentials);
    oauth::run_refresh_transaction(async move {
        let refreshed = refresh.await?;
        Ok(commit_refreshed_openai_tokens(&credentials, refreshed).await)
    })
    .await
}

async fn commit_refreshed_openai_tokens(
    credentials: &Arc<RwLock<CodexCredentials>>,
    refreshed: oauth::OAuthTokens,
) -> String {
    let mut tokens = credentials.write().await;
    let account_id = tokens.account_id.clone();
    let id_token = refreshed
        .id_token
        .clone()
        .or_else(|| tokens.id_token.clone());
    let new_access_token = refreshed.access_token.clone();

    *tokens = CodexCredentials {
        access_token: new_access_token.clone(),
        refresh_token: refreshed.refresh_token,
        id_token,
        account_id,
        expires_at: Some(refreshed.expires_at),
    };

    new_access_token
}

/// Privacy-safe context for a websocket stall: the provider response id (a
/// correlation handle, not content) and how long the stream had been silent.
fn ws_stall_diagnostics(
    response_id: Option<&str>,
    started_at: Instant,
    last_activity_at: Instant,
) -> String {
    format!(
        "response_id={} elapsed_ms={} since_last_activity_ms={}",
        response_id.unwrap_or("none"),
        started_at.elapsed().as_millis(),
        last_activity_at.elapsed().as_millis()
    )
}

/// Stream the response from OpenAI API
#[allow(clippy::too_many_arguments)]
fn response_http_request(
    client: &Client,
    url: &str,
    access_token: &str,
    is_chatgpt_mode: bool,
    account_id: Option<&str>,
    cache_key: Option<&str>,
    model: Option<&str>,
    service_tier: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut builder = client
        .post(url)
        .header("Authorization", format!("Bearer {access_token}"));
    if let Some((name, value)) =
        openai_websocket_prewarm::routing_header(is_chatgpt_mode, model, service_tier)
    {
        builder = builder.header(name, value);
    }
    if is_chatgpt_mode {
        builder = builder.header("originator", ORIGINATOR);
        if let Some(key) = cache_key {
            builder = builder.header("session-id", key);
        }
        if let Some(account_id) = account_id {
            builder = builder.header("chatgpt-account-id", account_id);
        }
    }
    if let Some(built) = builder.try_clone().and_then(|builder| builder.build().ok()) {
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "https_request_routing_headers",
            routing_header_log_fields(built.headers()),
        );
    }
    builder
}

fn routing_header_log_fields(headers: &reqwest::header::HeaderMap) -> Vec<(&'static str, String)> {
    [
        ("service_tier_header", "OpenAI-Service-Tier"),
        ("codex_routing_hint", "x-codex-routing-hint"),
    ]
    .into_iter()
    .map(|(field, header)| {
        (
            field,
            headers
                .get(header)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("none")
                .to_owned(),
        )
    })
    .collect()
}

/// Stream the response from OpenAI API
pub(super) async fn stream_response(
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    account_label: Option<String>,
    request: Value,
    initial_status_detail: String,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<(), OpenAIStreamFailure> {
    use jcode_message_types::ConnectionPhase;
    let requested_tier = request
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let request_model = openai_request_model(&request);
    let stream_started_at = Instant::now();
    log_openai_stream_lifecycle(
        jcode_base::logging::LogLevel::Info,
        "https_request_start",
        vec![
            ("model", request_model.clone()),
            ("transport", "https".to_string()),
        ],
    );
    let (usage_diag, usage_exhausted) = serving_account_diag(&*credentials.read().await);
    jcode_base::logging::info(&format!(
        "OpenAI limit diag: starting fresh HTTPS request usage=({usage_diag})"
    ));
    emit_status_detail(&tx, initial_status_detail).await;
    emit_connection_phase(&tx, ConnectionPhase::Authenticating).await;
    if tx.is_closed() {
        return Ok(());
    }
    let access_token = openai_access_token(account_label.clone(), &credentials).await?;
    if tx.is_closed() {
        return Ok(());
    }
    let creds = credentials.read().await;
    // Account switching can race token refresh. Never combine an old bearer
    // with a new account header or attribute that request to the new account.
    if access_token != creds.access_token {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "OpenAI credentials changed before request, retrying"
        )));
    }
    let mut usage_recorder = OAuthUsageRecorder::capture(&creds, &request);
    let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    let catalog_scope = OpenAIProvider::catalog_scope_for(&creds);
    let url = OpenAIProvider::responses_url(&creds);
    let account_id = creds.account_id.clone();
    drop(creds);

    let builder = response_http_request(
        &client,
        &url,
        &access_token,
        is_chatgpt_mode,
        account_id.as_deref(),
        request.get("prompt_cache_key").and_then(Value::as_str),
        request.get("model").and_then(Value::as_str),
        request.get("service_tier").and_then(Value::as_str),
    );

    emit_connection_phase(&tx, ConnectionPhase::SendingRequest).await;
    let connect_start = std::time::Instant::now();
    let idle_timeout = effective_https_idle_timeout(&request);
    let builder = jcode_provider_core::request_compression::json_body(
        builder,
        &url,
        &request,
        jcode_base::provider::request_compression_mode(),
    )
    .map_err(OpenAIStreamFailure::Other)?;

    let response = while_consumer_open(
        &tx,
        jcode_provider_core::transport::send_with_initial_response_timeout(builder, idle_timeout),
    )
    .await;
    let Some(response) = response else {
        return Ok(());
    };
    let response = response
        .context("Failed to send request to OpenAI API")
        .map_err(OpenAIStreamFailure::Other)?;

    let connect_ms = connect_start.elapsed().as_millis();
    jcode_base::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));
    // Correlation ids let a stalled request be looked up provider-side
    // without logging any content (issue #1759). Carried into every later
    // lifecycle event for this stream.
    let response_ids = reqwest_header_fields(response.headers());
    let mut connected_fields = vec![
        ("model", request_model.clone()),
        ("status", response.status().as_u16().to_string()),
        ("connect_ms", connect_ms.to_string()),
    ];
    connected_fields.extend(response_ids.iter().cloned());
    log_openai_stream_lifecycle(
        jcode_base::logging::LogLevel::Info,
        "https_connected",
        connected_fields,
    );
    if response.status().is_success() && usage_exhausted {
        jcode_base::logging::warn(&format!(
            "OpenAI limit diag: fresh HTTPS request accepted while the serving account's usage indicates exhausted usage=({usage_diag})"
        ));
    }

    if !response.status().is_success() {
        let status = response.status();
        let retry_after = jcode_provider_core::retry_after::retry_after(response.headers());

        let Some(body) = while_consumer_open(
            &tx,
            jcode_base::util::http_error_body(response, "HTTP error"),
        )
        .await
        else {
            return Ok(());
        };
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Warn,
            "https_http_error",
            vec![
                ("model", request_model.clone()),
                ("status", status.as_u16().to_string()),
                (
                    "retry_after_secs",
                    retry_after
                        .map(|hint| hint.remaining().as_secs().to_string())
                        .unwrap_or_else(|| "none".to_string()),
                ),
                ("body", body.clone()),
                (
                    "elapsed_ms",
                    stream_started_at.elapsed().as_millis().to_string(),
                ),
            ],
        );

        if let Some(reason) = classify_unavailable_model_error(status, &body)
            && let Some(model_name) = request.get("model").and_then(|m| m.as_str())
        {
            jcode_base::provider::record_model_unavailable_for_scope(
                &catalog_scope,
                model_name,
                &reason,
            );
            jcode_base::logging::warn(&format!(
                "Recorded OpenAI model '{}' as unavailable: {}",
                model_name, reason
            ));
        }

        // Check if we need to refresh token
        if should_refresh_token(status, &body) {
            if tx.is_closed() {
                return Ok(());
            }
            // The server rejected our access token (401/403). Proactively
            // refresh it in place so the retry loop reconnects with a fresh
            // token instead of surfacing a raw "Token refresh needed" error.
            let refresh_token = {
                let creds = credentials.read().await;
                creds.refresh_token.clone()
            };

            if refresh_token.is_empty() {
                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "OpenAI rejected the access token and no refresh token is available; run /login to re-authenticate: {}",
                    body
                )));
            }

            match force_refresh_openai_token(&credentials, &refresh_token, account_label.clone())
                .await
            {
                Ok(_) => {
                    jcode_base::logging::info(
                        "OpenAI access token rejected; refreshed credentials and will retry",
                    );
                    // Surface a retryable error so the retry loop reconnects
                    // with the freshly refreshed token.
                    return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                        "openai token refreshed, retrying: {}",
                        body
                    )));
                }
                Err(refresh_err) => {
                    return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                        "OpenAI token refresh failed; run /login to re-authenticate: {refresh_err:#}"
                    )));
                }
            }
        }

        // For rate limits, format structured payloads into a readable message.
        let msg = if status == StatusCode::TOO_MANY_REQUESTS {
            format_rate_limit_error(&body, retry_after.map(|hint| hint.remaining()))
        } else {
            format!("OpenAI API error {}: {}", status, body)
        };
        return Err(OpenAIStreamFailure::Other(
            jcode_provider_core::retry_after::error_with_retry_after(msg, retry_after),
        ));
    }

    emit_connection_phase(&tx, ConnectionPhase::WaitingForResponse).await;

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "https/sse".to_string(),
        }))
        .await;

    // Stream the response
    let mut stream = OpenAIResponsesStream::new(response.bytes_stream());
    let mut saw_message_end = false;
    let mut progress = StreamProgress::new();

    // Idle timeout between streamed events. Without this, a silently dead
    // connection (or a provider that never emits) would hang forever; with a
    // hardcoded short value, slow reasoning models that think silently for
    // minutes get cancelled prematurely. Resolved from
    // `[provider] stream_idle_timeout_secs` / `JCODE_STREAM_IDLE_TIMEOUT_SECS`
    // (issue #434).
    use futures::StreamExt;
    loop {
        let Some(next) =
            while_consumer_open(&tx, tokio::time::timeout(idle_timeout, stream.next())).await
        else {
            return Ok(());
        };
        let result = match next {
            Ok(Some(result)) => result,
            Ok(None) => break, // stream ended normally
            Err(_) => {
                let mut fields = vec![
                    ("model", request_model.clone()),
                    ("idle_timeout_secs", idle_timeout.as_secs().to_string()),
                    (
                        "elapsed_ms",
                        stream_started_at.elapsed().as_millis().to_string(),
                    ),
                ];
                fields.extend(progress.fields());
                fields.extend(response_ids.iter().cloned());
                log_openai_stream_lifecycle(
                    jcode_base::logging::LogLevel::Warn,
                    "https_stream_idle_timeout",
                    fields,
                );
                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Stream read timeout: no data received for {} seconds",
                    idle_timeout.as_secs()
                )));
            }
        };
        match result {
            Ok(mut event) => {
                bind_requested_service_tier(&mut event, &requested_tier);
                progress.record_event(&event);
                if matches!(event, StreamEvent::MessageEnd { .. }) {
                    saw_message_end = true;
                }
                if let StreamEvent::Error { message, .. } = &event {
                    if let Some(model_name) = request.get("model").and_then(|m| m.as_str()) {
                        maybe_record_runtime_model_unavailable_from_stream_error(
                            &catalog_scope,
                            model_name,
                            message,
                        );
                    }
                    if is_retryable_error(&message.to_lowercase()) {
                        log_openai_stream_lifecycle(
                            jcode_base::logging::LogLevel::Warn,
                            "https_stream_retryable_error",
                            vec![
                                ("model", request_model.clone()),
                                ("error", message.clone()),
                                (
                                    "elapsed_ms",
                                    stream_started_at.elapsed().as_millis().to_string(),
                                ),
                            ],
                        );
                        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                            "Stream error: {}",
                            message
                        )));
                    }
                }
                usage_recorder.observe(&event).await;
                if tx.send(Ok(event)).await.is_err() {
                    // Receiver dropped, stop streaming
                    log_openai_stream_lifecycle(
                        jcode_base::logging::LogLevel::Warn,
                        "consumer_dropped",
                        vec![
                            ("model", request_model.clone()),
                            ("transport", "https".to_string()),
                            (
                                "elapsed_ms",
                                stream_started_at.elapsed().as_millis().to_string(),
                            ),
                        ],
                    );
                    return Ok(());
                }
            }
            Err(e) => {
                let mut fields = vec![
                    ("model", request_model.clone()),
                    ("error", e.to_string()),
                    (
                        "elapsed_ms",
                        stream_started_at.elapsed().as_millis().to_string(),
                    ),
                ];
                fields.extend(progress.fields());
                fields.extend(response_ids.iter().cloned());
                log_openai_stream_lifecycle(
                    jcode_base::logging::LogLevel::Warn,
                    "https_stream_error",
                    fields,
                );
                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Stream error: {}",
                    e
                )));
            }
        }
    }

    if !saw_message_end {
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Warn,
            "https_eof_before_message_end",
            vec![
                ("model", request_model.clone()),
                (
                    "elapsed_ms",
                    stream_started_at.elapsed().as_millis().to_string(),
                ),
            ],
        );
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "OpenAI HTTPS stream ended before message completion marker"
        )));
    }

    log_openai_stream_lifecycle(
        jcode_base::logging::LogLevel::Info,
        "https_stream_complete",
        vec![
            ("model", request_model),
            (
                "elapsed_ms",
                stream_started_at.elapsed().as_millis().to_string(),
            ),
        ],
    );
    Ok(())
}

pub(super) fn is_ws_upgrade_required(err: &WsError) -> bool {
    match err {
        WsError::Http(response) => response.status() == WEBSOCKET_UPGRADE_REQUIRED_ERROR,
        _ => false,
    }
}

/// Result of trying to continue on a persistent WebSocket connection
pub(super) enum PersistentWsResult {
    Success,
    /// A terminal API error was forwarded to the consumer. Do not replay it.
    TerminalError,
    NotAvailable,
    Failed(String),
}

/// Try to continue a conversation on an existing persistent WebSocket connection
/// using `previous_response_id` to send only incremental input. A socket drained
/// after cancellation has no response id and receives a full, independent replay.
pub(super) async fn try_persistent_ws_continuation(
    persistent_ws: &Arc<Mutex<Option<PersistentWsState>>>,
    credentials: &Arc<RwLock<CodexCredentials>>,
    request: &Value,
    input: &[Value],
    input_item_count: usize,
    tx: &mpsc::Sender<Result<StreamEvent>>,
) -> PersistentWsResult {
    let mut guard = Arc::clone(persistent_ws).lock_owned().await;
    let mut attempt = RecoverableAttempt {
        state: guard.take(),
        phase: RecoveryPhase::Idle,
        slot: Some(guard),
    };
    let result = continue_persistent_ws_locked(
        &mut attempt.state,
        credentials,
        request,
        input,
        input_item_count,
        tx,
        &mut attempt.phase,
    )
    .await;
    // Invalidate under the same lock that protected the attempt. Clearing in
    // the caller after unlocking lets a queued request reuse the failed chain
    // and can later erase a replacement connection belonging to another turn.
    if matches!(
        result,
        PersistentWsResult::Failed(_) | PersistentWsResult::TerminalError
    ) {
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Warn,
            "persistent_state_reset",
            vec![
                ("model", openai_request_model(request)),
                ("reason", "persistent_reuse_failed".to_string()),
            ],
        );
        attempt.phase = RecoveryPhase::Idle;
    } else if attempt.phase == RecoveryPhase::Idle {
        **attempt.slot.as_mut().unwrap() = attempt.state.take();
    }
    result
}

async fn continue_persistent_ws_locked(
    guard: &mut Option<PersistentWsState>,
    credentials: &Arc<RwLock<CodexCredentials>>,
    request: &Value,
    input: &[Value],
    input_item_count: usize,
    tx: &mpsc::Sender<Result<StreamEvent>>,
    recovery_phase: &mut RecoveryPhase,
) -> PersistentWsResult {
    let request_model = openai_request_model(request);
    let state = match guard.as_mut() {
        Some(s) => s,
        None => {
            log_openai_stream_lifecycle(
                jcode_base::logging::LogLevel::Info,
                "persistent_reuse_unavailable_detail",
                vec![
                    ("model", request_model.clone()),
                    ("reason", "no_state".to_string()),
                ],
            );
            return PersistentWsResult::NotAvailable;
        }
    };

    if state.identity
        != openai_websocket_prewarm::prewarm_identity(
            &*credentials.read().await,
            request.get("model").and_then(Value::as_str),
            request.get("service_tier").and_then(Value::as_str),
        )
    {
        *guard = None;
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model.clone()),
                ("reason", "handshake_identity_changed".to_string()),
            ],
        );
        return PersistentWsResult::NotAvailable;
    }

    if state.cache_key.as_deref() != request.get("prompt_cache_key").and_then(Value::as_str) {
        *guard = None;
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model.clone()),
                ("reason", "cache_session_changed".to_string()),
            ],
        );
        return PersistentWsResult::NotAvailable;
    }

    // Check connection age - reconnect before the 60-min server limit
    if state.connected_at.elapsed() >= Duration::from_secs(WEBSOCKET_PERSISTENT_MAX_AGE_SECS) {
        jcode_base::logging::info("Persistent WS connection too old; forcing reconnect");
        *guard = None;
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model.clone()),
                ("reason", "max_age".to_string()),
                (
                    "max_age_secs",
                    WEBSOCKET_PERSISTENT_MAX_AGE_SECS.to_string(),
                ),
            ],
        );
        return PersistentWsResult::NotAvailable;
    }

    if persistent_ws_idle_needs_healthcheck(state.last_activity_at.elapsed()) {
        emit_status_detail(tx, "checking websocket").await;
    }

    match ensure_persistent_ws_is_healthy(state, true).await {
        Ok(PersistentWsHealth::Healthy) => {}
        Ok(PersistentWsHealth::Reconnect {
            reset_reason,
            detail,
        }) => {
            jcode_base::logging::info(&format!(
                "Persistent WS healthcheck requested reconnect before reuse: {}",
                detail
            ));
            *guard = None;
            log_openai_stream_lifecycle(
                jcode_base::logging::LogLevel::Info,
                "persistent_state_reset",
                vec![
                    ("model", request_model.clone()),
                    ("reason", reset_reason.to_string()),
                    ("source", "request_reuse".to_string()),
                    ("detail", detail),
                ],
            );
            return PersistentWsResult::NotAvailable;
        }
        Err(err) => {
            jcode_base::logging::warn(&format!(
                "Persistent WS healthcheck failed: {}; forcing reconnect",
                err
            ));
            *guard = None;
            log_openai_stream_lifecycle(
                jcode_base::logging::LogLevel::Warn,
                "persistent_state_reset",
                vec![
                    ("model", request_model.clone()),
                    ("reason", "healthcheck_failed".to_string()),
                    ("source", "request_reuse".to_string()),
                    ("error", err.to_string()),
                ],
            );
            return PersistentWsResult::NotAvailable;
        }
    }

    // The input array must be strictly growing for continuation to make sense.
    // If the input_item_count is less than or equal to last time, the conversation
    // was reset (e.g., after compaction) - we need a fresh connection.
    let full_replay = state.last_response_id.is_empty()
        || has_unanswered_tool_calls(&state.last_response_output, input);
    if !full_replay && input_item_count <= state.last_input_item_count {
        let last_input_item_count = state.last_input_item_count;
        jcode_base::logging::info(&format!(
            "Input items didn't grow ({} <= {}); conversation may have been compacted, reconnecting",
            input_item_count, last_input_item_count
        ));
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model.clone()),
                ("reason", "input_not_growing".to_string()),
                ("input_item_count", input_item_count.to_string()),
                ("last_input_item_count", last_input_item_count.to_string()),
            ],
        );
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    // Canonicalization can move a new tool output before the old cursor, even
    // when the input grows. Slicing by count would silently omit that output.
    // Replay the full normalized history unless the prior input is unchanged.
    let input_item_hashes = persistent_ws_input_item_hashes(input);
    if !full_replay
        && (state.last_input_item_hashes.len() != state.last_input_item_count
            || !input_item_hashes.starts_with(&state.last_input_item_hashes))
    {
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model.clone()),
                ("reason", "input_prefix_changed".to_string()),
                ("input_item_count", input_item_count.to_string()),
                (
                    "last_input_item_count",
                    state.last_input_item_count.to_string(),
                ),
            ],
        );
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    // Compute incremental items: everything after the last_input_item_count.
    //
    // When continuing with `previous_response_id`, OpenAI already has every
    // output item produced by that previous response, including native
    // reasoning store items (`rs_...`). Replaying those items in the next delta
    // makes the API reject the request with "Duplicate item found with id
    // rs_...". The full input still needs reasoning items for fresh requests,
    // but deltas must only contain genuinely new client-side input/tool
    // callbacks.
    let (incremental_items, skipped_output_items) = if full_replay || state.message_count == 0 {
        // A generate:false warmup has no model output in its context. Preserve
        // all history (including encrypted reasoning) for its first generation.
        (input.to_vec(), 0)
    } else {
        persistent_ws_incremental_items(
            input,
            state.last_input_item_count,
            &state.last_response_output,
        )
    };
    if skipped_output_items > 0 {
        jcode_base::logging::info(&format!(
            "Skipped {} prior response output item(s) in persistent WS continuation delta",
            skipped_output_items
        ));
    }
    if incremental_items.is_empty() {
        jcode_base::logging::info("No incremental items to send; need fresh request");
        *guard = None;
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model.clone()),
                ("reason", "empty_incremental_items".to_string()),
            ],
        );
        return PersistentWsResult::NotAvailable;
    }

    let requested_tier = request
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let incremental_stats = summarize_ws_input(&incremental_items);
    if state.message_count > 0 && incremental_stats.function_call_items > 0 {
        log_ws_replayed_calls(&incremental_items, &state.last_response_output);
    }
    let previous_response_id = if full_replay {
        String::new()
    } else {
        state.last_response_id.clone()
    };
    let request_prompt_cache_key_hash = request
        .get("prompt_cache_key")
        .map(jcode_provider_core::fingerprint::stable_hash_json);
    let (usage_diag, usage_exhausted) = serving_account_diag(&*credentials.read().await);
    jcode_base::logging::info(&format!(
        "OpenAI limit diag: attempting persistent WS reuse previous_response_id_present={} usage=({}) state=({})",
        !previous_response_id.is_empty(),
        usage_diag,
        state.diag_snapshot().log_fields()
    ));
    jcode_base::logging::info(&format!(
        "Persistent WS continuation: previous_response_id={} {} tool_callback={} (was {} now {})",
        previous_response_id,
        incremental_stats.log_fields(),
        incremental_stats.tool_callback_count() > 0,
        state.last_input_item_count,
        input_item_count,
    ));
    log_openai_stream_lifecycle(
        jcode_base::logging::LogLevel::Info,
        "persistent_reuse_start",
        vec![
            ("model", request_model.clone()),
            ("transport", "websocket".to_string()),
            ("input_item_count", input_item_count.to_string()),
            (
                "last_input_item_count",
                state.last_input_item_count.to_string(),
            ),
            (
                "incremental_item_count",
                incremental_items.len().to_string(),
            ),
            (
                "previous_response_id_present",
                (!previous_response_id.is_empty()).to_string(),
            ),
            (
                "tool_callback",
                (incremental_stats.tool_callback_count() > 0).to_string(),
            ),
            (
                "request_kind",
                if full_replay {
                    "ws_cancel_full_replay"
                } else {
                    "ws_delta"
                }
                .to_string(),
            ),
            (
                "cache_namespace",
                if full_replay {
                    "full_request"
                } else {
                    "previous_response_delta"
                }
                .to_string(),
            ),
            (
                "prompt_cache_key_present",
                request.get("prompt_cache_key").is_some().to_string(),
            ),
            (
                "prompt_cache_key_hash",
                format!("{:?}", request_prompt_cache_key_hash),
            ),
            (
                "prompt_cache_retention",
                request
                    .get("prompt_cache_retention")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
            ),
            (
                "service_tier",
                request
                    .get("service_tier")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
            ),
        ],
    );

    // Build the incremental request - only include new items + previous_response_id
    let mut continuation_request = serde_json::json!({
        "type": "response.create",
        "previous_response_id": previous_response_id,
        "input": incremental_items,
    });

    // Copy over model, tools, and other settings from the original request
    if let Some(model) = request.get("model") {
        continuation_request["model"] = model.clone();
    }
    if let Some(tools) = request.get("tools") {
        continuation_request["tools"] = tools.clone();
    }
    if let Some(tool_choice) = request.get("tool_choice") {
        continuation_request["tool_choice"] = tool_choice.clone();
    }
    if let Some(instructions) = request.get("instructions") {
        continuation_request["instructions"] = instructions.clone();
    }
    if let Some(max_output_tokens) = request.get("max_output_tokens") {
        continuation_request["max_output_tokens"] = max_output_tokens.clone();
    }
    if let Some(reasoning) = request.get("reasoning") {
        continuation_request["reasoning"] = reasoning.clone();
    }
    if let Some(context_management) = request.get("context_management") {
        continuation_request["context_management"] = context_management.clone();
    }
    if let Some(include) = request.get("include") {
        continuation_request["include"] = include.clone();
    }
    if let Some(service_tier) = request.get("service_tier") {
        continuation_request["service_tier"] = service_tier.clone();
    }
    if let Some(prompt_cache_key) = request.get("prompt_cache_key") {
        continuation_request["prompt_cache_key"] = prompt_cache_key.clone();
    }
    if let Some(prompt_cache_retention) = request.get("prompt_cache_retention") {
        continuation_request["prompt_cache_retention"] = prompt_cache_retention.clone();
    }
    if let Some(options) = request.get("prompt_cache_options") {
        continuation_request["prompt_cache_options"] = options.clone();
    }
    if !full_replay && state.message_count > 0 {
        add_cache_comparison(
            &mut continuation_request,
            OpenAIProvider::is_chatgpt_mode(&*credentials.read().await),
            &previous_response_id,
        );
    }
    continuation_request["store"] = serde_json::json!(false);
    continuation_request["parallel_tool_calls"] = serde_json::json!(false);

    if full_replay {
        // A drained cancelled response may contain unseen tool calls. Reuse
        // only its transport, never its server-side conversation context.
        continuation_request = request.clone();
        let Some(object) = continuation_request.as_object_mut() else {
            return PersistentWsResult::Failed("invalid full replay request".into());
        };
        object.insert("type".into(), serde_json::json!("response.create"));
        object.insert("input".into(), Value::Array(input.to_vec()));
        object.remove("previous_response_id");
        object.remove("stream");
        object.remove("background");
    }

    let continuation_tools = continuation_request
        .get("tools")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let continuation_instructions = continuation_request.get("instructions").cloned();
    let continuation_tool_count = continuation_tools
        .as_array()
        .map(|tools| tools.len())
        .unwrap_or(0);
    let prompt_cache_key_hash = continuation_request
        .get("prompt_cache_key")
        .map(jcode_provider_core::fingerprint::stable_hash_json);
    let model_for_fingerprint = continuation_request
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let continuation_payload = serde_json::json!({
        "type": continuation_request.get("type"),
        "previous_response_id_hash": jcode_provider_core::fingerprint::stable_hash_str(&previous_response_id),
        "model": continuation_request.get("model"),
        "instructions": continuation_request.get("instructions"),
        "input": &incremental_items,
        "tools": continuation_request.get("tools"),
        "tool_choice": continuation_request.get("tool_choice"),
        "parallel_tool_calls": continuation_request.get("parallel_tool_calls"),
        "reasoning": continuation_request.get("reasoning"),
        "context_management": continuation_request.get("context_management"),
        "include": continuation_request.get("include"),
        "service_tier": continuation_request.get("service_tier"),
        "prompt_cache_key": continuation_request.get("prompt_cache_key"),
        "prompt_cache_retention": continuation_request.get("prompt_cache_retention"),
    });
    jcode_provider_core::fingerprint::log_provider_canonical_input(
        "openai",
        model_for_fingerprint,
        if full_replay {
            "openai_responses_ws_cancel_full_replay"
        } else {
            "openai_responses_ws_delta"
        },
        &continuation_payload,
        &incremental_items,
        continuation_instructions.as_ref(),
        Some(&continuation_tools),
        Some(continuation_tool_count),
        &[
            (
                "previous_response_id_present",
                (!previous_response_id.is_empty()).to_string(),
            ),
            ("input_item_count", input_item_count.to_string()),
            (
                "last_input_item_count",
                state.last_input_item_count.to_string(),
            ),
            (
                "incremental_item_count",
                incremental_items.len().to_string(),
            ),
            (
                "request_kind",
                if full_replay {
                    "ws_cancel_full_replay"
                } else {
                    "ws_delta"
                }
                .to_string(),
            ),
            (
                "cache_namespace",
                if full_replay {
                    "full_request"
                } else {
                    "previous_response_delta"
                }
                .to_string(),
            ),
            ("transport_mode", "websocket".to_string()),
            (
                "prompt_cache_key_present",
                continuation_request
                    .get("prompt_cache_key")
                    .is_some()
                    .to_string(),
            ),
            (
                "prompt_cache_key_hash",
                format!("{:?}", prompt_cache_key_hash),
            ),
            (
                "prompt_cache_retention",
                continuation_request
                    .get("prompt_cache_retention")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
            ),
            (
                "service_tier",
                continuation_request
                    .get("service_tier")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
            ),
        ],
    );

    let request_text = match serde_json::to_string(&continuation_request) {
        Ok(t) => t,
        Err(e) => return PersistentWsResult::Failed(format!("serialize error: {}", e)),
    };

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "websocket/persistent-reuse".to_string(),
        }))
        .await;

    // Send the continuation request on the existing WebSocket
    let send_started_at = Instant::now();
    emit_connection_phase(tx, jcode_message_types::ConnectionPhase::SendingRequest).await;
    // Health checks and event backpressure above can yield to a credential
    // change in another fork. Revalidate at the send boundary and keep the
    // read guard until the frame is flushed, not throughout generation.
    let send_credentials = credentials.read().await;
    if state.identity
        != openai_websocket_prewarm::prewarm_identity(
            &send_credentials,
            request.get("model").and_then(Value::as_str),
            request.get("service_tier").and_then(Value::as_str),
        )
    {
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }
    let mut usage_recorder = OAuthUsageRecorder::capture(&send_credentials, request);
    let ws_send_timeout = effective_https_idle_timeout(&continuation_request);
    if let Err(e) = send_websocket_frame(
        &mut state.ws_stream,
        WsMessage::Text(request_text),
        ws_send_timeout,
    )
    .await
    {
        return PersistentWsResult::Failed(format!("send error: {}", e));
    }
    drop(send_credentials);
    *recovery_phase = RecoveryPhase::InFlight;
    emit_connection_phase(tx, jcode_message_types::ConnectionPhase::WaitingForResponse).await;
    state.last_activity_at = Instant::now();
    jcode_base::logging::info(&format!(
        "Persistent WS continuation request sent in {}ms ({})",
        send_started_at.elapsed().as_millis(),
        incremental_stats.log_fields(),
    ));

    // Stream the response, extracting the new response_id
    let mut saw_text_delta = false;
    let mut saw_thinking_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut consumer_dropped = false;
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();
    let mut new_response_id: Option<String> = None;
    let mut response_output = Vec::new();
    let stream_started = Instant::now();
    let mut last_api_activity_at = stream_started;
    let mut saw_api_activity = false;
    let mut logged_first_server_event = false;
    let ws_completion_timeout_secs = effective_ws_completion_timeout_secs(&continuation_request);

    loop {
        // Only inactivity ends the stream. A wall-clock cap here used to kill
        // healthy long responses (large tool-call arguments, long answers)
        // after five minutes and replay them from scratch (issue #1759).
        let timeout_secs = match websocket_next_activity_timeout_secs_with_completion(
            stream_started,
            last_api_activity_at,
            saw_api_activity,
            ws_completion_timeout_secs,
        ) {
            Some(timeout_secs) => timeout_secs,
            None => {
                return PersistentWsResult::Failed(format!(
                    "timed out waiting for {} websocket activity on persistent WS ({}s; {})",
                    websocket_activity_timeout_kind(saw_api_activity),
                    if saw_api_activity {
                        ws_completion_timeout_secs
                    } else {
                        WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                    },
                    ws_stall_diagnostics(
                        new_response_id.as_deref(),
                        stream_started,
                        last_api_activity_at
                    ),
                ));
            }
        };
        let next_item =
            match tokio::time::timeout(Duration::from_secs(timeout_secs), state.ws_stream.next())
                .await
            {
                Ok(item) => item,
                Err(_) => {
                    return PersistentWsResult::Failed(format!(
                        "timed out waiting for {} websocket activity on persistent WS ({}s; {})",
                        websocket_activity_timeout_kind(saw_api_activity),
                        timeout_secs,
                        ws_stall_diagnostics(
                            new_response_id.as_deref(),
                            stream_started,
                            last_api_activity_at
                        ),
                    ));
                }
            };

        let Some(result) = next_item else {
            if saw_response_completed {
                break;
            }
            return PersistentWsResult::Failed(
                "persistent WS stream ended before response.completed".to_string(),
            );
        };

        match result {
            Ok(WsMessage::Text(text)) => {
                let text = text.to_string();
                if text.contains("response.completed")
                    && serde_json::from_str::<Value>(&text)
                        .ok()
                        .is_some_and(|event| {
                            event.get("type").and_then(Value::as_str) == Some("response.completed")
                        })
                {
                    *recovery_phase = RecoveryPhase::Completed;
                }
                if !logged_first_server_event {
                    emit_connection_phase(tx, jcode_message_types::ConnectionPhase::Streaming)
                        .await;
                    jcode_base::logging::info(&format!(
                        "Persistent WS first server event after {}ms ({})",
                        stream_started.elapsed().as_millis(),
                        incremental_stats.log_fields(),
                    ));
                    logged_first_server_event = true;
                }
                if is_websocket_fallback_notice(&text) {
                    return PersistentWsResult::Failed("server requested fallback".to_string());
                }

                let mut made_api_activity = if saw_api_activity {
                    is_websocket_activity_payload(&text)
                } else {
                    is_websocket_first_activity_payload(&text)
                };

                observe_ws_response_output(&text, &mut response_output);

                // Extract response_id from response.created event
                if new_response_id.is_none()
                    && let Ok(val) = serde_json::from_str::<Value>(&text)
                    && val.get("type").and_then(|t| t.as_str()) == Some("response.created")
                    && let Some(id) = val
                        .get("response")
                        .and_then(|r| r.get("id"))
                        .and_then(|id| id.as_str())
                {
                    new_response_id = Some(id.to_string());
                    jcode_base::logging::info(&format!(
                        "Persistent WS got new response_id after {}ms: {} ({})",
                        stream_started.elapsed().as_millis(),
                        id,
                        incremental_stats.log_fields(),
                    ));
                    if usage_exhausted {
                        jcode_base::logging::warn(&format!(
                            "OpenAI limit diag: persistent WS reuse accepted request while the serving account's usage indicates exhausted usage=({}) state=({})",
                            usage_diag,
                            state.diag_snapshot().log_fields()
                        ));
                    }
                }

                if let Some(mut event) = parse_openai_response_event(
                    &text,
                    &mut saw_text_delta,
                    &mut saw_thinking_delta,
                    &mut streaming_tool_calls,
                    &mut completed_tool_items,
                    &mut pending,
                ) {
                    bind_requested_service_tier(&mut event, &requested_tier);
                    if is_stream_activity_event(&event) {
                        made_api_activity = true;
                    }
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        saw_response_completed = true;
                    }
                    if let StreamEvent::Error { ref message, .. } = event {
                        let lower = message.to_lowercase();
                        if is_retryable_error(&lower)
                            || lower.contains("previous_response_not_found")
                            || lower.contains("no tool output found for function call")
                        {
                            return PersistentWsResult::Failed(format!(
                                "stream error: {}",
                                message
                            ));
                        }
                        // A failed response will not send response.completed.
                        // Forward once and stop, even if the server keeps the
                        // socket open or the consumer retains its stream.
                        let _ = tx.send(Ok(event)).await;
                        return PersistentWsResult::TerminalError;
                    }
                    usage_recorder.observe(&event).await;
                    if tx.send(Ok(event)).await.is_err() {
                        consumer_dropped = true;
                        break;
                    }
                }
                while let Some(mut event) = pending.pop_front() {
                    bind_requested_service_tier(&mut event, &requested_tier);
                    if let StreamEvent::Error { ref message, .. } = event {
                        let lower = message.to_lowercase();
                        if is_retryable_error(&lower)
                            || lower.contains("previous_response_not_found")
                            || lower.contains("no tool output found for function call")
                        {
                            return PersistentWsResult::Failed(format!(
                                "stream error: {}",
                                message
                            ));
                        }
                        let _ = tx.send(Ok(event)).await;
                        return PersistentWsResult::TerminalError;
                    }
                    if is_stream_activity_event(&event) {
                        made_api_activity = true;
                    }
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        saw_response_completed = true;
                    }
                    usage_recorder.observe(&event).await;
                    if tx.send(Ok(event)).await.is_err() {
                        consumer_dropped = true;
                        break;
                    }
                }
                if consumer_dropped {
                    break;
                }
                if made_api_activity {
                    saw_api_activity = true;
                    let now = Instant::now();
                    last_api_activity_at = now;
                    state.last_activity_at = now;
                }
                if saw_response_completed {
                    break;
                }
            }
            Ok(WsMessage::Ping(payload)) => {
                if let Err(error) = send_websocket_frame(
                    &mut state.ws_stream,
                    WsMessage::Pong(payload),
                    Duration::from_millis(WEBSOCKET_PERSISTENT_HEALTHCHECK_TIMEOUT_MS),
                )
                .await
                {
                    return PersistentWsResult::Failed(format!("pong send error: {error}"));
                }
                state.last_activity_at = Instant::now();
            }
            Ok(WsMessage::Close(_)) => {
                if saw_response_completed {
                    break;
                }
                return PersistentWsResult::Failed("server closed connection".to_string());
            }
            Ok(WsMessage::Pong(_)) | Ok(_) => {}
            Err(e) => {
                return PersistentWsResult::Failed(format!("ws error: {}", e));
            }
        }
    }

    // A soft interrupt may leave unseen tool calls, even when completion has
    // already arrived. The owner will bounded-drain if needed and preserve only
    // the transport, never this response's implicit continuation context.
    if consumer_dropped || !saw_response_completed {
        jcode_base::logging::info(
            "Persistent WS consumer dropped; recovering transport without response chain",
        );
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_state_reset",
            vec![
                ("model", request_model),
                (
                    "reason",
                    "consumer_dropped_response_chain_discarded".to_string(),
                ),
                ("consumer_dropped", consumer_dropped.to_string()),
            ],
        );
        return PersistentWsResult::Success;
    }

    // Update persistent state for next turn
    if let Some(resp_id) = new_response_id {
        state.last_response_id = resp_id;
        state.last_input_item_count = input_item_count;
        state.last_input_item_hashes = input_item_hashes;
        state.last_response_output = response_output;
        state.message_count += 1;
        state.last_activity_at = Instant::now();
        state.last_response_completed_at = Instant::now();
        jcode_base::logging::info(&format!(
            "Persistent WS continuation success after {}ms (chain length: {}, {})",
            stream_started.elapsed().as_millis(),
            state.message_count,
            incremental_stats.log_fields(),
        ));
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "persistent_reuse_stream_complete",
            vec![
                ("model", request_model),
                ("transport", "websocket".to_string()),
                ("chain_length", state.message_count.to_string()),
                (
                    "elapsed_ms",
                    stream_started.elapsed().as_millis().to_string(),
                ),
            ],
        );
        *recovery_phase = RecoveryPhase::Idle;
        PersistentWsResult::Success
    } else {
        // Got response but no response_id - can't chain further
        jcode_base::logging::warn("Persistent WS: no response_id in response; breaking chain");
        *guard = None;
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Warn,
            "persistent_state_reset",
            vec![
                ("model", request_model),
                ("reason", "missing_response_id".to_string()),
                (
                    "elapsed_ms",
                    stream_started.elapsed().as_millis().to_string(),
                ),
            ],
        );
        PersistentWsResult::Success
    }
}

/// Stream response via WebSocket, saving the connection for reuse.
/// This replaces the old `stream_response_websocket` for the fresh-connection path.
pub(super) async fn stream_response_websocket_persistent(
    credentials: Arc<RwLock<CodexCredentials>>,
    account_label: Option<String>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    persistent_ws: Arc<Mutex<Option<PersistentWsState>>>,
    input_item_count: usize,
) -> Result<(), OpenAIStreamFailure> {
    let request_credentials = Arc::clone(&credentials);
    let result = after_authentication(
        &tx,
        openai_access_token(account_label, &credentials),
        |access_token| {
            stream_response_websocket_authenticated(
                request_credentials,
                request,
                tx.clone(),
                persistent_ws,
                input_item_count,
                access_token,
            )
        },
    )
    .await?;
    result.unwrap_or(Ok(()))
}

async fn stream_response_websocket_authenticated(
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    persistent_ws: Arc<Mutex<Option<PersistentWsState>>>,
    input_item_count: usize,
    access_token: String,
) -> Result<(), OpenAIStreamFailure> {
    use jcode_message_types::ConnectionPhase;
    let requested_tier = request
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let request_model = request
        .get("model")
        .and_then(|m| m.as_str())
        .map(|m| m.to_string());
    let request_model_label = request_model
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let stream_started_at = Instant::now();
    log_openai_stream_lifecycle(
        jcode_base::logging::LogLevel::Info,
        "fresh_ws_request_start",
        vec![
            ("model", request_model_label.clone()),
            ("transport", "websocket".to_string()),
            ("input_item_count", input_item_count.to_string()),
        ],
    );

    let (usage_diag, usage_exhausted) = serving_account_diag(&*credentials.read().await);
    jcode_base::logging::info(&format!(
        "OpenAI limit diag: opening fresh persistent WS request usage=({usage_diag})"
    ));
    emit_status_detail(&tx, "opening websocket").await;
    let creds = credentials.read().await;
    // Account switching can race token refresh. Never combine an old bearer
    // with a new account header or attribute that request to the new account.
    if access_token != creds.access_token {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "OpenAI credentials changed before request, retrying"
        )));
    }
    let mut usage_recorder = OAuthUsageRecorder::capture(&creds, &request);
    let catalog_scope = OpenAIProvider::catalog_scope_for(&creds);
    let ws_request = openai_websocket_prewarm::websocket_request(
        &creds,
        &access_token,
        request.get("prompt_cache_key").and_then(Value::as_str),
        request.get("model").and_then(Value::as_str),
        request.get("service_tier").and_then(Value::as_str),
    )
    .map_err(OpenAIStreamFailure::Other)?;
    let mut identity = openai_websocket_prewarm::prewarm_identity(
        &creds,
        request.get("model").and_then(Value::as_str),
        request.get("service_tier").and_then(Value::as_str),
    );
    log_openai_stream_lifecycle(
        jcode_base::logging::LogLevel::Info,
        "fresh_ws_request_routing_headers",
        routing_header_log_fields(ws_request.headers()),
    );
    identity.0 = access_token;
    drop(creds);

    emit_connection_phase(&tx, ConnectionPhase::Connecting).await;
    let connect_start = std::time::Instant::now();

    let connect_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_CONNECT_TIMEOUT_SECS),
        connect_ws(ws_request),
    )
    .await
    .map_err(|_| {
        OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
            "WebSocket connect timed out after {}s",
            WEBSOCKET_CONNECT_TIMEOUT_SECS
        ))
    })?;

    let (mut ws_stream, _response) = match connect_result {
        Ok((stream, response)) => {
            let connect_ms = connect_start.elapsed().as_millis();
            jcode_base::logging::info(&format!(
                "WebSocket connection established in {}ms (persistent mode)",
                connect_ms
            ));
            let mut fields = vec![
                ("model", request_model_label.clone()),
                ("connect_ms", connect_ms.to_string()),
            ];
            // Handshake correlation ids identify the socket provider-side.
            fields.extend(tungstenite_header_fields(&response));
            log_openai_stream_lifecycle(
                jcode_base::logging::LogLevel::Info,
                "fresh_ws_connected",
                fields,
            );
            (stream, response)
        }
        Err(err) if is_ws_upgrade_required(&err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "Falling back from websockets to HTTPS transport"
            )));
        }
        Err(err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "Failed to connect websocket stream: {}",
                err
            )));
        }
    };

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "websocket/persistent-fresh".to_string(),
        }))
        .await;

    let input_item_hashes = persistent_ws_input_item_hashes(
        request
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    );
    let cache_key = request
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut request_event = request;
    if !request_event.is_object() {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Invalid websocket request payload shape; expected an object"
        )));
    }
    {
        let Some(obj) = request_event.as_object_mut() else {
            return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                "Invalid websocket request payload shape; expected an object"
            )));
        };
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );
        obj.remove("stream");
        obj.remove("background");
    }

    let request_input_stats = request_event
        .get("input")
        .and_then(|value| value.as_array())
        .map(|items| summarize_ws_input(items))
        .unwrap_or_default();

    let request_text = serde_json::to_string(&request_event).map_err(|err| {
        OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Failed to serialize OpenAI websocket request: {}",
            err
        ))
    })?;
    let request_send_started_at = Instant::now();
    emit_connection_phase(&tx, ConnectionPhase::SendingRequest).await;
    let ws_send_timeout = effective_https_idle_timeout(&request_event);
    send_websocket_frame(
        &mut ws_stream,
        WsMessage::Text(request_text),
        ws_send_timeout,
    )
    .await
    .map_err(|err| OpenAIStreamFailure::Other(anyhow::anyhow!(err)))?;
    emit_connection_phase(&tx, ConnectionPhase::WaitingForResponse).await;
    jcode_base::logging::info(&format!(
        "Fresh WS request sent in {}ms ({})",
        request_send_started_at.elapsed().as_millis(),
        request_input_stats.log_fields(),
    ));

    let mut saw_text_delta = false;
    let mut saw_thinking_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut saw_api_activity = false;
    let ws_started_at = Instant::now();
    let mut last_api_activity_at = ws_started_at;
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();
    let mut response_id: Option<String> = None;
    let mut response_output = Vec::new();
    let connected_at = Instant::now();
    let mut logged_first_server_event = false;
    let ws_completion_timeout_secs = effective_ws_completion_timeout_secs(&request_event);

    loop {
        // Only inactivity ends the stream (see the persistent path above):
        // no wall-clock cap on a response that is still producing events.
        if !saw_api_activity
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not emit API activity within {}s",
                WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
            )));
        }

        let timeout_secs = websocket_next_activity_timeout_secs_with_completion(
            ws_started_at,
            last_api_activity_at,
            saw_api_activity,
            ws_completion_timeout_secs,
        )
        .ok_or_else(|| {
            OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream timed out waiting for {} websocket activity ({}s; {})",
                websocket_activity_timeout_kind(saw_api_activity),
                if saw_api_activity {
                    ws_completion_timeout_secs
                } else {
                    WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                },
                ws_stall_diagnostics(response_id.as_deref(), ws_started_at, last_api_activity_at),
            ))
        })?;
        let next_item = tokio::time::timeout(Duration::from_secs(timeout_secs), ws_stream.next())
            .await
            .map_err(|_| {
                OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                    "WebSocket stream timed out waiting for {} websocket activity ({}s; {})",
                    websocket_activity_timeout_kind(saw_api_activity),
                    timeout_secs,
                    ws_stall_diagnostics(
                        response_id.as_deref(),
                        ws_started_at,
                        last_api_activity_at
                    ),
                ))
            })?;

        let Some(result) = next_item else {
            if saw_response_completed {
                break;
            }
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream ended before response.completed"
            )));
        };

        match result {
            Ok(message) => match message {
                WsMessage::Text(text) => {
                    let text = text.to_string();
                    if !logged_first_server_event {
                        emit_connection_phase(&tx, ConnectionPhase::Streaming).await;
                        jcode_base::logging::info(&format!(
                            "Fresh WS first server event after {}ms ({})",
                            ws_started_at.elapsed().as_millis(),
                            request_input_stats.log_fields(),
                        ));
                        logged_first_server_event = true;
                    }
                    if is_websocket_fallback_notice(&text) {
                        return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                            "{} reported by websocket stream",
                            WEBSOCKET_FALLBACK_NOTICE
                        )));
                    }

                    observe_ws_response_output(&text, &mut response_output);

                    // Extract response_id from response.created event
                    if response_id.is_none()
                        && let Ok(val) = serde_json::from_str::<Value>(&text)
                        && val.get("type").and_then(|t| t.as_str()) == Some("response.created")
                        && let Some(id) = val
                            .get("response")
                            .and_then(|r| r.get("id"))
                            .and_then(|id| id.as_str())
                    {
                        response_id = Some(id.to_string());
                        jcode_base::logging::info(&format!(
                            "Fresh WS got response_id after {}ms: {} (will save for continuation; {})",
                            ws_started_at.elapsed().as_millis(),
                            id,
                            request_input_stats.log_fields(),
                        ));
                        if usage_exhausted {
                            jcode_base::logging::warn(&format!(
                                "OpenAI limit diag: fresh WS request accepted while the serving account's usage indicates exhausted usage=({usage_diag})"
                            ));
                        }
                    }

                    let mut made_api_activity = if saw_api_activity {
                        is_websocket_activity_payload(&text)
                    } else {
                        is_websocket_first_activity_payload(&text)
                    };
                    if let Some(mut event) = parse_openai_response_event(
                        &text,
                        &mut saw_text_delta,
                        &mut saw_thinking_delta,
                        &mut streaming_tool_calls,
                        &mut completed_tool_items,
                        &mut pending,
                    ) {
                        bind_requested_service_tier(&mut event, &requested_tier);
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    &catalog_scope,
                                    model_name,
                                    message,
                                );
                            }
                            if is_retryable_error(&message.to_lowercase()) {
                                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                                    "Stream error: {}",
                                    message
                                )));
                            }
                            // A failed response never sends response.completed,
                            // and the server may keep the socket open (usage
                            // limits do). Forward once and drop the socket.
                            let _ = tx.send(Ok(event)).await;
                            return Ok(());
                        }
                        usage_recorder.observe(&event).await;
                        if tx.send(Ok(event)).await.is_err() {
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Warn,
                                "consumer_dropped",
                                vec![
                                    ("model", request_model_label.clone()),
                                    ("transport", "websocket".to_string()),
                                    (
                                        "elapsed_ms",
                                        stream_started_at.elapsed().as_millis().to_string(),
                                    ),
                                ],
                            );
                            return Ok(());
                        }
                    }
                    while let Some(mut event) = pending.pop_front() {
                        bind_requested_service_tier(&mut event, &requested_tier);
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    &catalog_scope,
                                    model_name,
                                    message,
                                );
                            }
                            if is_retryable_error(&message.to_lowercase()) {
                                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                                    "Stream error: {}",
                                    message
                                )));
                            }
                            let _ = tx.send(Ok(event)).await;
                            return Ok(());
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        usage_recorder.observe(&event).await;
                        if tx.send(Ok(event)).await.is_err() {
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Warn,
                                "consumer_dropped",
                                vec![
                                    ("model", request_model_label.clone()),
                                    ("transport", "websocket".to_string()),
                                    (
                                        "elapsed_ms",
                                        stream_started_at.elapsed().as_millis().to_string(),
                                    ),
                                ],
                            );
                            return Ok(());
                        }
                    }
                    if made_api_activity {
                        saw_api_activity = true;
                        last_api_activity_at = Instant::now();
                    }
                    if saw_response_completed {
                        break;
                    }
                }
                WsMessage::Ping(payload) => {
                    send_websocket_frame(
                        &mut ws_stream,
                        WsMessage::Pong(payload),
                        Duration::from_millis(WEBSOCKET_PERSISTENT_HEALTHCHECK_TIMEOUT_MS),
                    )
                    .await
                    .map_err(|err| OpenAIStreamFailure::Other(anyhow::anyhow!(err)))?;
                }
                WsMessage::Close(_) => {
                    if saw_response_completed {
                        break;
                    }
                    return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                        "WebSocket stream closed before response.completed"
                    )));
                }
                WsMessage::Binary(_) => {
                    return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                        "Unexpected binary websocket event"
                    )));
                }
                WsMessage::Pong(_) => {}
                _ => {}
            },
            Err(err) => {
                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Stream error: {}",
                    err
                )));
            }
        }
    }

    // Save the WebSocket connection and response_id for reuse on next turn
    if let Some(resp_id) = response_id {
        let mut guard = persistent_ws.lock().await;
        jcode_base::logging::info(&format!(
            "Saving persistent WS connection after {}ms (response_id={}, {})",
            ws_started_at.elapsed().as_millis(),
            resp_id,
            request_input_stats.log_fields(),
        ));
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "fresh_ws_stream_complete_saved",
            vec![
                ("model", request_model_label.clone()),
                ("transport", "websocket".to_string()),
                (
                    "elapsed_ms",
                    stream_started_at.elapsed().as_millis().to_string(),
                ),
                ("response_id_present", "true".to_string()),
            ],
        );
        *guard = Some(PersistentWsState {
            cache_key,
            ws_stream,
            identity,
            last_response_id: resp_id,
            connected_at,
            last_activity_at: Instant::now(),
            last_response_completed_at: Instant::now(),
            message_count: 1,
            last_input_item_count: input_item_count,
            last_input_item_hashes: input_item_hashes,
            last_response_output: response_output,
        });
        drop(guard);
        spawn_persistent_ws_keepalive(
            Arc::downgrade(&persistent_ws),
            connected_at,
            request_model_label.clone(),
        );
    } else {
        jcode_base::logging::info(
            "No response_id captured from WS stream; connection not saved for reuse",
        );
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Warn,
            "fresh_ws_stream_complete_not_saved",
            vec![
                ("model", request_model_label),
                ("transport", "websocket".to_string()),
                (
                    "elapsed_ms",
                    stream_started_at.elapsed().as_millis().to_string(),
                ),
                ("reason", "missing_response_id".to_string()),
            ],
        );
    }

    Ok(())
}

fn should_refresh_token(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::UNAUTHORIZED {
        return true;
    }
    if status == StatusCode::FORBIDDEN {
        let lower = body.to_lowercase();
        return lower.contains("token")
            || lower.contains("expired")
            || lower.contains("unauthorized");
    }
    false
}

fn maybe_record_runtime_model_unavailable_from_stream_error(
    catalog_scope: &str,
    model: &str,
    message: &str,
) {
    let reason = classify_unavailable_model_error(StatusCode::BAD_REQUEST, message)
        .or_else(|| classify_unavailable_model_error(StatusCode::FORBIDDEN, message));

    if let Some(reason) = reason {
        jcode_base::provider::record_model_unavailable_for_scope(catalog_scope, model, &reason);
        jcode_base::logging::warn(&format!(
            "Recorded OpenAI model '{}' as unavailable from stream error: {}",
            model, reason
        ));
    }
}

fn classify_unavailable_model_error(status: StatusCode, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();

    let mentions_model = lower.contains("model")
        || lower.contains("slug")
        || lower.contains("engine")
        || lower.contains("deployment");
    let unavailable = lower.contains("not available")
        || lower.contains("unavailable")
        || lower.contains("does not have access")
        || lower.contains("not enabled")
        || lower.contains("not found")
        || lower.contains("unknown model")
        || lower.contains("unsupported model")
        || lower.contains("invalid model");

    if !mentions_model || !unavailable {
        return None;
    }

    if status == StatusCode::NOT_FOUND
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::BAD_REQUEST
        || status == StatusCode::UNPROCESSABLE_ENTITY
    {
        let trimmed = body.trim();
        let reason = if trimmed.is_empty() {
            format!("model denied by OpenAI API (status {})", status)
        } else {
            format!(
                "model denied by OpenAI API (status {}): {}",
                status, trimmed
            )
        };
        return Some(reason);
    }

    None
}

/// Check if an error is transient and should be retried
pub(super) fn is_retryable_error(error_str: &str) -> bool {
    // Shared transport-layer classifier used by every other provider. This
    // covers transient TLS/network faults (connection reset/closed/refused/
    // aborted, broken pipe, timeouts, unexpected EOF, error decoding/reading,
    // TLS BadRecordMac / fatal-alert, TLS handshake EOF, DNS/route failures,
    // and HTTP/2 stream/protocol faults). Keeping the OpenAI path delegated
    // here ensures retry behavior is unified across providers (issue #338).
    jcode_provider_core::is_transient_transport_error(error_str)
        // OpenAI-specific transport wrapper.
        || error_str.contains("failed to send request to openai api")
        // Stream/decode errors specific to the OpenAI streaming runtime.
        || error_str.contains("incomplete message")
        || error_str.contains("stream disconnected before completion")
        || error_str.contains("ended before message completion marker")
        || error_str.contains("falling back from websockets to https transport")
        // Server errors (5xx)
        || error_str.contains("500 internal server error")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
        // Rate limiting (429): transient, recovers on retry. Unified with the
        // other providers (Anthropic/Copilot) which already retry these.
        || error_str.contains("429 too many requests")
        || error_str.contains("rate limit")
        || error_str.contains("rate_limit")
        // API-level server errors
        || error_str.contains("api_error")
        || error_str.contains("server_error")
        || error_str.contains("internal server error")
        || error_str.contains("an error occurred while processing your request")
        || error_str.contains("please include the request id")
        // Auth: we just force-refreshed the OpenAI token in place and want the
        // retry loop to reconnect with the fresh credentials.
        || error_str.contains("openai token refreshed, retrying")
        || error_str.contains("openai credentials changed before request, retrying")
}

#[cfg(test)]
#[path = "openai_stream_runtime/stream_runtime_tests.rs"]
mod stream_runtime_tests;
