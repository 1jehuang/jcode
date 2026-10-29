//! Attribute completed Messages responses to the credential used for the request.
use super::*;
use crate::sse_types::UsageInfo;

pub(super) fn update_usage(state: &mut SseStreamState, usage: UsageInfo) {
    state.output_tokens = usage.output_tokens.map(u64::from).or(state.output_tokens);
    state.input_tokens = usage.input_tokens.map(u64::from).or(state.input_tokens);
    state.cache_read_input_tokens = usage
        .cache_read_input_tokens
        .map(u64::from)
        .or(state.cache_read_input_tokens);
    state.cache_creation_input_tokens = usage
        .cache_creation_input_tokens
        .map(u64::from)
        .or(state.cache_creation_input_tokens);
    if let Some(tier) = usage.service_tier {
        jcode_base::logging::info(&format!("Anthropic granted service_tier={tier}"));
        if std::env::var("JCODE_LOG_SERVICE_TIER").is_ok() {
            eprintln!("[anthropic] granted service_tier={tier}");
        }
        state.service_tier = Some(tier);
    }
    // Fast mode is reported as `usage.speed`; record it as the fast tier so
    // usage and cost tracking bill fast-mode rates.
    if usage.speed.as_deref() == Some("fast") {
        jcode_base::logging::info("Anthropic granted speed=fast");
        state.service_tier = Some("auto".to_string());
    }
    if let Some(creation) = usage.cache_creation {
        state.cache_creation_5m = creation
            .ephemeral_5m_input_tokens
            .or(state.cache_creation_5m);
        state.cache_creation_1h = creation
            .ephemeral_1h_input_tokens
            .or(state.cache_creation_1h);
    }
}

pub(super) async fn send_final_usage(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    state: &SseStreamState,
) {
    if state.input_tokens.is_some() || state.output_tokens.is_some() {
        if state.cache_read_input_tokens.is_some() || state.cache_creation_input_tokens.is_some() {
            jcode_base::logging::info(&format!(
                "Prompt cache: read={:?} created={:?}",
                state.cache_read_input_tokens, state.cache_creation_input_tokens
            ));
        }
        if tx
            .send(Ok(StreamEvent::TokenUsage {
                input_tokens: state.input_tokens,
                output_tokens: state.output_tokens,
                cache_read_input_tokens: state.cache_read_input_tokens,
                cache_creation_input_tokens: state.cache_creation_input_tokens,
            }))
            .await
            .is_err()
        {
            jcode_base::logging::debug(
                "Anthropic terminal usage consumer disconnected after accounting",
            );
        }
    }
}

fn matching_label(token: &str, accounts: &[auth::claude::AnthropicAccount]) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let mut matches = accounts.iter().filter(|account| account.access == token);
    if let Some(account) = matches.next() {
        return matches.next().is_none().then(|| account.label.clone());
    }
    // Legacy credentials without named accounts are exposed as the default login.
    accounts.is_empty().then(|| "default".to_string())
}

pub(super) fn credential_label(is_oauth: bool, token: &str) -> Option<String> {
    if !is_oauth {
        return None;
    }
    let label = match auth::claude::list_accounts() {
        Ok(accounts) => matching_label(token, &accounts),
        Err(error) => {
            jcode_base::logging::warn(&format!(
                "Cannot load Anthropic accounts for usage attribution: {error}"
            ));
            return None;
        }
    };
    if label.is_none() {
        jcode_base::logging::warn(
            "Anthropic usage not recorded: request credential could not be attributed to a unique account",
        );
    }
    label
}

pub(super) async fn record(label: Option<&str>, requested_model: &str, state: &SseStreamState) {
    let Some(label) = label else { return };
    let label = label.to_string();
    let model = state
        .served_model
        .as_deref()
        .unwrap_or(requested_model)
        .to_string();
    let (input, output, read, created) = (
        state.input_tokens,
        state.output_tokens,
        state.cache_read_input_tokens,
        state.cache_creation_input_tokens,
    );
    let tier = state.service_tier.clone();
    let (cache_5m, cache_1h) = (state.cache_creation_5m, state.cache_creation_1h);
    // Complete the durable write before forwarding terminal usage, independently
    // of the TUI consumer. Never charge incremental message_start/delta twice.
    if let Err(error) = tokio::task::spawn_blocking(move || {
        jcode_base::provider_activity::record_anthropic_oauth_usage(
            &label,
            &model,
            input,
            output,
            read,
            created,
            tier.as_deref(),
            cache_5m,
            cache_1h,
        );
    })
    .await
    {
        jcode_base::logging::warn(&format!("Anthropic usage recorder task failed: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(label: &str, token: &str) -> auth::claude::AnthropicAccount {
        auth::claude::AnthropicAccount {
            label: label.into(),
            access: token.into(),
            refresh: String::new(),
            expires: i64::MAX,
            email: None,
            subscription_type: None,
            scopes: vec![],
        }
    }

    #[test]
    fn terminal_http_stream_records_once_and_only_for_oauth() {
        use std::io::{Read, Write};
        let _lock = jcode_base::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => jcode_base::env::set_var("JCODE_HOME", value),
                    None => jcode_base::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _restore = Restore(std::env::var_os("JCODE_HOME"));
        jcode_base::env::set_var("JCODE_HOME", home.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (label, terminal) in [
            (Some("captured"), true),
            (None, true),
            (Some("incomplete"), false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/messages", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let read = socket.read(&mut chunk).unwrap();
                    assert!(read > 0);
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let mut body = concat!(
                    "event: message_start\ndata: {\"message\":{\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":100,\"cache_read_input_tokens\":40,\"cache_creation_input_tokens\":30,\"cache_creation\":{\"ephemeral_5m_input_tokens\":10,\"ephemeral_1h_input_tokens\":20}}}}\n\n",
                    "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":20}}\n\n"
                ).to_string();
                if terminal {
                    // Duplicate terminal frames must not duplicate accounting.
                    body.push_str(
                        "event: message_stop\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
                    );
                }
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            runtime.block_on(async {
                let (tx, mut rx) = mpsc::channel(100);
                let mut transport = DirectTransportConfig::from_env();
                transport.api_url = endpoint.clone();
                transport.auth_mode = "none".into();
                transport.oauth_api_url = Some(endpoint);
                let request = ApiRequest {
                    model: "unpriced-requested-alias".into(),
                    max_tokens: 1024,
                    system: None,
                    messages: vec![],
                    tools: None,
                    metadata: None,
                    thinking: None,
                    output_config: None,
                    temperature: None,
                    service_tier: None,
                    stream: true,
                };
                stream_response(
                    Client::builder().no_proxy().build().unwrap(),
                    "token".into(),
                    label.is_some(),
                    request,
                    tx,
                    "unpriced-requested-alias",
                    "test-session",
                    &transport,
                    &mut false,
                    label,
                )
                .await
                .unwrap();
                let mut usage_events = 0;
                while let Some(event) = rx.recv().await {
                    if matches!(event.unwrap(), StreamEvent::TokenUsage { .. }) {
                        usage_events += 1;
                    }
                }
                assert_eq!(usage_events, 1);
            });
            server.join().unwrap();
        }
        let rows = jcode_base::provider_activity::anthropic_oauth_usage_summary("captured");
        let today = &rows.iter().find(|(key, _)| key == "Today").unwrap().1;
        assert!(
            today.contains("170 in · 20 out · 40 cached · 30 cache creation"),
            "{today}"
        );
        assert!(
            !today.contains("unknown"),
            "served model should determine price: {today}"
        );
        assert!(
            jcode_base::provider_activity::anthropic_oauth_usage_summary("incomplete")[0]
                .1
                .contains("No recorded usage")
        );
        assert!(!home.path().join("provider_activity.json").exists());
        assert!(!home.path().join("openai_oauth_usage.json").exists());
    }

    #[test]
    fn cumulative_usage_deltas_preserve_missing_fields() {
        let mut state = SseStreamState::default();
        for data in [
            r#"{"delta":{},"usage":{"input_tokens":100,"output_tokens":2,"cache_read_input_tokens":40,"cache_creation_input_tokens":30,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20},"service_tier":"standard"}}"#,
            r#"{"delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":120,"output_tokens":20}}"#,
        ] {
            process_sse_event(
                &SseEvent {
                    event_type: "message_delta".into(),
                    data: data.into(),
                },
                &mut state,
                true,
            );
        }
        assert_eq!(state.input_tokens, Some(120));
        assert_eq!(state.output_tokens, Some(20));
        assert_eq!(state.cache_read_input_tokens, Some(40));
        assert_eq!(state.cache_creation_input_tokens, Some(30));
        assert_eq!(state.cache_creation_5m, Some(10));
        assert_eq!(state.cache_creation_1h, Some(20));
        assert_eq!(state.service_tier.as_deref(), Some("standard"));
    }

    #[test]
    fn usage_speed_fast_records_fast_tier() {
        let mut state = SseStreamState::default();
        process_sse_event(
            &SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"delta":{},"usage":{"input_tokens":5,"output_tokens":2,"speed":"fast"}}"#
                    .into(),
            },
            &mut state,
            true,
        );
        assert_eq!(state.service_tier.as_deref(), Some("auto"));
    }

    #[test]
    fn matches_request_credential_not_active_account() {
        let accounts = vec![account("personal", "a"), account("work", "b")];
        assert_eq!(matching_label("b", &accounts).as_deref(), Some("work"));
        assert_eq!(matching_label("unknown", &accounts), None);
        assert_eq!(matching_label("", &[]), None);
        assert_eq!(matching_label("legacy", &[]).as_deref(), Some("default"));
        assert_eq!(
            matching_label("a", &[account("one", "a"), account("two", "a")]),
            None
        );
        assert_eq!(credential_label(false, "b"), None);
    }
}
