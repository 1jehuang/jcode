use super::*;

pub(crate) async fn stream_response(
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

    if tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "https/sse".to_string(),
        }))
        .await
        .is_err()
    {
        return Ok(());
    }

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
