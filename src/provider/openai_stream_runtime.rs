use super::*;

pub(super) async fn openai_access_token(
    credentials: &Arc<RwLock<CodexCredentials>>,
) -> anyhow::Result<String> {
    let (access_token, refresh_token, needs_refresh) = {
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
        )
    };

    if !needs_refresh {
        return Ok(access_token);
    }

    if refresh_token.is_empty() {
        return Ok(access_token);
    }

    let refreshed = oauth::refresh_openai_tokens(&refresh_token).await?;
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

    Ok(new_access_token)
}

/// Stream the response from OpenAI API
pub(super) async fn stream_response(
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    initial_status_detail: String,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<(), OpenAIStreamFailure> {
    use crate::message::ConnectionPhase;
    let usage_snapshot = crate::usage::get_openai_usage_sync();
    crate::logging::info(&format!(
        "OpenAI limit diag: starting fresh HTTPS request usage=({})",
        usage_snapshot.diagnostic_fields()
    ));
    emit_status_detail(&tx, initial_status_detail).await;
    emit_connection_phase(&tx, ConnectionPhase::Authenticating).await;
    let access_token = openai_access_token(&credentials).await?;
    let creds = credentials.read().await;
    let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    let url = OpenAIProvider::responses_url(&creds);
    let account_id = creds.account_id.clone();
    drop(creds);

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json");

    if is_chatgpt_mode {
        builder = builder.header("originator", ORIGINATOR);
        if let Some(account_id) = account_id.as_ref() {
            builder = builder.header("chatgpt-account-id", account_id);
        }
    }

    emit_connection_phase(&tx, ConnectionPhase::Connecting).await;
    let connect_start = std::time::Instant::now();

    let response = builder
        .json(&request)
        .send()
        .await
        .context("Failed to send request to OpenAI API")
        .map_err(OpenAIStreamFailure::Other)?;

    let connect_ms = connect_start.elapsed().as_millis();
    crate::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));
    if response.status().is_success() && usage_snapshot.exhausted() {
        crate::logging::warn(&format!(
            "OpenAI limit diag: fresh HTTPS request accepted while local usage indicates exhausted usage=({})",
            usage_snapshot.diagnostic_fields()
        ));
    }

    if !response.status().is_success() {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let body = crate::util::http_error_body(response, "HTTP error").await;

        if let Some(reason) = classify_unavailable_model_error(status, &body)
            && let Some(model_name) = request.get("model").and_then(|m| m.as_str())
        {
            crate::provider::record_model_unavailable_for_account(model_name, &reason);
            crate::logging::warn(&format!(
                "Recorded OpenAI model '{}' as unavailable: {}",
                model_name, reason
            ));
        }

        // Check if we need to refresh token
        if should_refresh_token(status, &body) {
            // Token refresh needed - this is a retryable error
            return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                "Token refresh needed: {}",
                body
            )));
        }

        // For rate limits, include retry info in the error
        let msg = if status == StatusCode::TOO_MANY_REQUESTS {
            let wait_info = retry_after
                .map(|s| format!(" (retry after {}s)", s))
                .unwrap_or_default();
            format!("Rate limited{}: {}", wait_info, body)
        } else {
            format!("OpenAI API error {}: {}", status, body)
        };
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!("{}", msg)));
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

    use futures::StreamExt;
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => {
                if matches!(event, StreamEvent::MessageEnd { .. }) {
                    saw_message_end = true;
                }
                if let StreamEvent::Error { message, .. } = &event {
                    if let Some(model_name) = request.get("model").and_then(|m| m.as_str()) {
                        maybe_record_runtime_model_unavailable_from_stream_error(
                            model_name, message,
                        );
                    }
                    if is_retryable_error(&message.to_lowercase()) {
                        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                            "Stream error: {}",
                            message
                        )));
                    }
                }
                if tx.send(Ok(event)).await.is_err() {
                    // Receiver dropped, stop streaming
                    return Ok(());
                }
            }
            Err(e) => {
                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Stream error: {}",
                    e
                )));
            }
        }
    }

    if !saw_message_end {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "OpenAI HTTPS stream ended before message completion marker"
        )));
    }

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
    NotAvailable,
    Failed(String),
}

/// Try to continue a conversation on an existing persistent WebSocket connection
/// using `previous_response_id` to send only incremental input.
pub(super) async fn try_persistent_ws_continuation(
    persistent_ws: &Arc<Mutex<Option<PersistentWsState>>>,
    request: &Value,
    input: &[Value],
    input_item_count: usize,
    tx: &mpsc::Sender<Result<StreamEvent>>,
) -> PersistentWsResult {
    let mut guard = persistent_ws.lock().await;
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PersistentWsResult::NotAvailable,
    };

    // Check connection age - reconnect before the 60-min server limit
    if state.connected_at.elapsed() >= Duration::from_secs(WEBSOCKET_PERSISTENT_MAX_AGE_SECS) {
        crate::logging::info("Persistent WS connection too old; forcing reconnect");
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    if persistent_ws_idle_needs_healthcheck(state.last_activity_at.elapsed()) {
        emit_status_detail(tx, "checking websocket").await;
    }

    match ensure_persistent_ws_is_healthy(state).await {
        Ok(true) => {}
        Ok(false) => {
            crate::logging::info("Persistent WS healthcheck requested reconnect before reuse");
            *guard = None;
            return PersistentWsResult::NotAvailable;
        }
        Err(err) => {
            crate::logging::warn(&format!(
                "Persistent WS healthcheck failed: {}; forcing reconnect",
                err
            ));
            *guard = None;
            return PersistentWsResult::NotAvailable;
        }
    }

    // The input array must be strictly growing for continuation to make sense.
    // If the input_item_count is less than or equal to last time, the conversation
    // was reset (e.g., after compaction) - we need a fresh connection.
    if input_item_count <= state.last_input_item_count {
        crate::logging::info(&format!(
            "Input items didn't grow ({} <= {}); conversation may have been compacted, reconnecting",
            input_item_count, state.last_input_item_count
        ));
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    // Compute incremental items: everything after the last_input_item_count
    let incremental_items: Vec<Value> = input[state.last_input_item_count..].to_vec();
    if incremental_items.is_empty() {
        crate::logging::info("No incremental items to send; need fresh request");
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    let incremental_stats = summarize_ws_input(&incremental_items);
    let previous_response_id = state.last_response_id.clone();
    let usage_snapshot = crate::usage::get_openai_usage_sync();
    crate::logging::info(&format!(
        "OpenAI limit diag: attempting persistent WS reuse previous_response_id_present={} usage=({}) state=({})",
        !previous_response_id.is_empty(),
        usage_snapshot.diagnostic_fields(),
        state.diag_snapshot().log_fields()
    ));
    crate::logging::info(&format!(
        "Persistent WS continuation: previous_response_id={} {} tool_callback={} (was {} now {})",
        previous_response_id,
        incremental_stats.log_fields(),
        incremental_stats.tool_callback_count() > 0,
        state.last_input_item_count,
        input_item_count,
    ));

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
    continuation_request["store"] = serde_json::json!(false);
    continuation_request["parallel_tool_calls"] = serde_json::json!(false);

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
    if let Err(e) = state.ws_stream.send(WsMessage::Text(request_text)).await {
        return PersistentWsResult::Failed(format!("send error: {}", e));
    }
    emit_connection_phase(tx, crate::message::ConnectionPhase::WaitingForResponse).await;
    state.last_activity_at = Instant::now();
    crate::logging::info(&format!(
        "Persistent WS continuation request sent in {}ms ({})",
        send_started_at.elapsed().as_millis(),
        incremental_stats.log_fields(),
    ));

    // Stream the response, extracting the new response_id
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();
    let mut new_response_id: Option<String> = None;
    let stream_started = Instant::now();
    let mut last_api_activity_at = stream_started;
    let mut saw_api_activity = false;
    let mut logged_first_server_event = false;

    loop {
        if stream_started.elapsed() >= Duration::from_secs(WEBSOCKET_COMPLETION_TIMEOUT_SECS) {
            return PersistentWsResult::Failed("completion timeout".to_string());
        }

        let timeout_secs = match websocket_next_activity_timeout_secs(
            stream_started,
            last_api_activity_at,
            saw_api_activity,
        ) {
            Some(timeout_secs) => timeout_secs,
            None => {
                return PersistentWsResult::Failed(format!(
                    "timed out waiting for {} websocket activity on persistent WS ({}s)",
                    websocket_activity_timeout_kind(saw_api_activity),
                    if saw_api_activity {
                        WEBSOCKET_COMPLETION_TIMEOUT_SECS
                    } else {
                        WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                    }
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
                        "timed out waiting for {} websocket activity on persistent WS ({}s)",
                        websocket_activity_timeout_kind(saw_api_activity),
                        timeout_secs
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
                if !logged_first_server_event {
                    emit_connection_phase(tx, crate::message::ConnectionPhase::Streaming).await;
                    crate::logging::info(&format!(
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
                    crate::logging::info(&format!(
                        "Persistent WS got new response_id after {}ms: {} ({})",
                        stream_started.elapsed().as_millis(),
                        id,
                        incremental_stats.log_fields(),
                    ));
                    let usage_snapshot = crate::usage::get_openai_usage_sync();
                    if usage_snapshot.exhausted() {
                        crate::logging::warn(&format!(
                            "OpenAI limit diag: persistent WS reuse accepted request while local usage indicates exhausted usage=({}) state=({})",
                            usage_snapshot.diagnostic_fields(),
                            state.diag_snapshot().log_fields()
                        ));
                    }
                }

                if let Some(event) = parse_openai_response_event(
                    &text,
                    &mut saw_text_delta,
                    &mut streaming_tool_calls,
                    &mut completed_tool_items,
                    &mut pending,
                ) {
                    if is_stream_activity_event(&event) {
                        made_api_activity = true;
                    }
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        saw_response_completed = true;
                    }
                    if let StreamEvent::Error { ref message, .. } = event
                        && is_retryable_error(&message.to_lowercase())
                    {
                        return PersistentWsResult::Failed(format!("stream error: {}", message));
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        break; // Receiver dropped
                    }
                }
                while let Some(event) = pending.pop_front() {
                    if is_stream_activity_event(&event) {
                        made_api_activity = true;
                    }
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        saw_response_completed = true;
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        break;
                    }
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
                let _ = state.ws_stream.send(WsMessage::Pong(payload)).await;
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

    // Update persistent state for next turn
    if let Some(resp_id) = new_response_id {
        state.last_response_id = resp_id;
        state.last_input_item_count = input_item_count;
        state.message_count += 1;
        state.last_activity_at = Instant::now();
        crate::logging::info(&format!(
            "Persistent WS continuation success after {}ms (chain length: {}, {})",
            stream_started.elapsed().as_millis(),
            state.message_count,
            incremental_stats.log_fields(),
        ));
        PersistentWsResult::Success
    } else {
        // Got response but no response_id - can't chain further
        crate::logging::warn("Persistent WS: no response_id in response; breaking chain");
        *guard = None;
        PersistentWsResult::Success
    }
}

/// Stream response via WebSocket, saving the connection for reuse.
/// This replaces the old `stream_response_websocket` for the fresh-connection path.
pub(super) async fn stream_response_websocket_persistent(
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    persistent_ws: Arc<Mutex<Option<PersistentWsState>>>,
    input_item_count: usize,
) -> Result<(), OpenAIStreamFailure> {
    use crate::message::ConnectionPhase;
    let request_model = request
        .get("model")
        .and_then(|m| m.as_str())
        .map(|m| m.to_string());

    let access_token = openai_access_token(&credentials).await?;
    let usage_snapshot = crate::usage::get_openai_usage_sync();
    crate::logging::info(&format!(
        "OpenAI limit diag: opening fresh persistent WS request usage=({})",
        usage_snapshot.diagnostic_fields()
    ));
    emit_status_detail(&tx, "opening websocket").await;
    let creds = credentials.read().await;
    let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    let ws_url = OpenAIProvider::responses_ws_url(&creds);
    let mut ws_request = ws_url.into_client_request().map_err(|err| {
        OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Failed to build websocket request: {}",
            err
        ))
    })?;

    let auth_header =
        HeaderValue::from_str(&format!("Bearer {}", access_token)).map_err(|err| {
            OpenAIStreamFailure::Other(anyhow::anyhow!("Invalid Authorization header: {}", err))
        })?;
    ws_request
        .headers_mut()
        .insert("Authorization", auth_header);
    ws_request
        .headers_mut()
        .insert("Content-Type", HeaderValue::from_static("application/json"));

    if is_chatgpt_mode {
        ws_request
            .headers_mut()
            .insert("originator", HeaderValue::from_static(ORIGINATOR));
        if let Some(account_id) = creds.account_id.as_ref() {
            let account_header = HeaderValue::from_str(account_id).map_err(|err| {
                OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Invalid chatgpt-account-id header: {}",
                    err
                ))
            })?;
            ws_request
                .headers_mut()
                .insert("chatgpt-account-id", account_header);
        }
    }
    drop(creds);

    emit_connection_phase(&tx, ConnectionPhase::Connecting).await;
    let connect_start = std::time::Instant::now();

    let connect_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_CONNECT_TIMEOUT_SECS),
        connect_async(ws_request),
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
            crate::logging::info(&format!(
                "WebSocket connection established in {}ms (persistent mode)",
                connect_ms
            ));
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
    ws_stream
        .send(WsMessage::Text(request_text))
        .await
        .map_err(|err| OpenAIStreamFailure::Other(anyhow::anyhow!(err)))?;
    emit_connection_phase(&tx, ConnectionPhase::WaitingForResponse).await;
    crate::logging::info(&format!(
        "Fresh WS request sent in {}ms ({})",
        request_send_started_at.elapsed().as_millis(),
        request_input_stats.log_fields(),
    ));

    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut saw_api_activity = false;
    let ws_started_at = Instant::now();
    let mut last_api_activity_at = ws_started_at;
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();
    let mut response_id: Option<String> = None;
    let connected_at = Instant::now();
    let mut logged_first_server_event = false;

    loop {
        if !saw_response_completed
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_COMPLETION_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not complete within {}s",
                WEBSOCKET_COMPLETION_TIMEOUT_SECS
            )));
        }

        if !saw_api_activity
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not emit API activity within {}s",
                WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
            )));
        }

        let timeout_secs = websocket_next_activity_timeout_secs(
            ws_started_at,
            last_api_activity_at,
            saw_api_activity,
        )
        .ok_or_else(|| {
            OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream timed out waiting for {} websocket activity ({}s)",
                websocket_activity_timeout_kind(saw_api_activity),
                if saw_api_activity {
                    WEBSOCKET_COMPLETION_TIMEOUT_SECS
                } else {
                    WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                }
            ))
        })?;
        let next_item = tokio::time::timeout(Duration::from_secs(timeout_secs), ws_stream.next())
            .await
            .map_err(|_| {
                OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                    "WebSocket stream timed out waiting for {} websocket activity ({}s)",
                    websocket_activity_timeout_kind(saw_api_activity),
                    timeout_secs
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
                        crate::logging::info(&format!(
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
                        crate::logging::info(&format!(
                            "Fresh WS got response_id after {}ms: {} (will save for continuation; {})",
                            ws_started_at.elapsed().as_millis(),
                            id,
                            request_input_stats.log_fields(),
                        ));
                        if usage_snapshot.exhausted() {
                            crate::logging::warn(&format!(
                                "OpenAI limit diag: fresh WS request accepted while local usage indicates exhausted usage=({})",
                                usage_snapshot.diagnostic_fields()
                            ));
                        }
                    }

                    let mut made_api_activity = if saw_api_activity {
                        is_websocket_activity_payload(&text)
                    } else {
                        is_websocket_first_activity_payload(&text)
                    };
                    if let Some(event) = parse_openai_response_event(
                        &text,
                        &mut saw_text_delta,
                        &mut streaming_tool_calls,
                        &mut completed_tool_items,
                        &mut pending,
                    ) {
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    model_name, message,
                                );
                            }
                            if is_retryable_error(&message.to_lowercase()) {
                                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                                    "Stream error: {}",
                                    message
                                )));
                            }
                        }
                        if tx.send(Ok(event)).await.is_err() {
                            return Ok(());
                        }
                    }
                    while let Some(event) = pending.pop_front() {
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    model_name, message,
                                );
                            }
                            if is_retryable_error(&message.to_lowercase()) {
                                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                                    "Stream error: {}",
                                    message
                                )));
                            }
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if tx.send(Ok(event)).await.is_err() {
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
                    let _ = ws_stream.send(WsMessage::Pong(payload)).await;
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
        crate::logging::info(&format!(
            "Saving persistent WS connection after {}ms (response_id={}, {})",
            ws_started_at.elapsed().as_millis(),
            resp_id,
            request_input_stats.log_fields(),
        ));
        *guard = Some(PersistentWsState {
            ws_stream,
            last_response_id: resp_id,
            connected_at,
            last_activity_at: Instant::now(),
            message_count: 1,
            last_input_item_count: input_item_count,
        });
    } else {
        crate::logging::info(
            "No response_id captured from WS stream; connection not saved for reuse",
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

fn maybe_record_runtime_model_unavailable_from_stream_error(model: &str, message: &str) {
    let reason = classify_unavailable_model_error(StatusCode::BAD_REQUEST, message)
        .or_else(|| classify_unavailable_model_error(StatusCode::FORBIDDEN, message));

    if let Some(reason) = reason {
        crate::provider::record_model_unavailable_for_account(model, &reason);
        crate::logging::warn(&format!(
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

pub(super) fn extract_error_with_retry(
    response: &Option<Value>,
    top_level_error: &Option<Value>,
) -> (String, Option<u64>) {
    // For "response.failed" events, the error is nested: response.error.message
    // For "error"/"response.error" events, the error is top-level: error.message
    let error = response
        .as_ref()
        .and_then(|r| r.get("error"))
        .or(top_level_error.as_ref());

    let error = match error {
        Some(e) => e,
        None => {
            // Last resort: check if response itself has a status_message or message
            if let Some(resp) = response.as_ref()
                && let Some(msg) = resp
                    .get("status_message")
                    .or_else(|| resp.get("message"))
                    .and_then(|v| v.as_str())
            {
                return (msg.to_string(), None);
            }
            return (
                "OpenAI response stream error (no error details)".to_string(),
                None,
            );
        }
    };

    let message = error
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("OpenAI response stream error (unknown)")
        .to_string();
    let error_type = error.get("type").and_then(|v| v.as_str());
    let code = error.get("code").and_then(|v| v.as_str());

    let message_lower = message.to_lowercase();
    let message = match (error_type, code) {
        (Some(error_type), Some(code))
            if !message_lower.contains(&error_type.to_lowercase())
                && !message_lower.contains(&code.to_lowercase()) =>
        {
            format!("{} ({}): {}", error_type, code, message)
        }
        (Some(error_type), _) if !message_lower.contains(&error_type.to_lowercase()) => {
            format!("{}: {}", error_type, message)
        }
        (_, Some(code)) if !message_lower.contains(&code.to_lowercase()) => {
            format!("{}: {}", code, message)
        }
        _ => message,
    };

    // Try to extract retry_after from error object or response metadata
    let retry_after = error
        .get("retry_after")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            response
                .as_ref()
                .and_then(|r| r.get("retry_after"))
                .and_then(|v| v.as_u64())
        });

    (message, retry_after)
}

/// Check if an error is transient and should be retried
pub(super) fn is_retryable_error(error_str: &str) -> bool {
    // Network/connection errors
    error_str.contains("connection reset")
        || error_str.contains("connection closed")
        || error_str.contains("connection refused")
        || error_str.contains("broken pipe")
        || error_str.contains("timed out")
        || error_str.contains("timeout")
        || error_str.contains("failed to send request to openai api")
        // Stream/decode errors
        || error_str.contains("error decoding")
        || error_str.contains("error reading")
        || error_str.contains("unexpected eof")
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
        // API-level server errors
        || error_str.contains("api_error")
        || error_str.contains("server_error")
        || error_str.contains("internal server error")
        || error_str.contains("an error occurred while processing your request")
        || error_str.contains("please include the request id")
}
