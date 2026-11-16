use super::*;
use jcode_message_types::ConnectionPhase;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, PartialEq)]
enum RetryDecision {
    Yield(String),
    Retry,
    Complete,
}

#[derive(Clone)]
struct ExhaustionApiTarget;

#[async_trait::async_trait]
impl jcode_provider_core::Provider for ExhaustionApiTarget {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        panic!("exhaustion target must only be proposed, never requested");
    }
    fn name(&self) -> &str {
        "OpenAI"
    }
    fn model(&self) -> String {
        "gpt-5.4".into()
    }
    fn credential_mode(&self) -> jcode_provider_core::CredentialMode {
        jcode_provider_core::CredentialMode::ApiKey
    }
    fn set_credential_mode(
        &self,
        _mode: jcode_provider_core::CredentialMode,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            model == "gpt-5.4",
            "unexpected exhaustion target model: {model}"
        );
        Ok(())
    }
    fn fork(&self) -> Arc<dyn jcode_provider_core::Provider> {
        Arc::new(self.clone())
    }
}

struct ExhaustionTargetGuard(Arc<std::sync::atomic::AtomicBool>);
impl Drop for ExhaustionTargetGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
struct RouterCase {
    split: bool,
    exhaust_alternate: bool,
    cached_first_rejection: bool,
}

// Exercise the real OAuth HTTP request and retry loop. A 60-second server
// backoff makes a missed yield fail the short timeout instead of passing after
// an ordinary exponential retry.
fn oauth_http_retry_decision(
    enabled: bool,
    alternate: bool,
    response: &str,
) -> (RetryDecision, usize, bool) {
    http_retry_decision(enabled, alternate, response, None, false)
}

fn http_retry_decision(
    enabled: bool,
    alternate: bool,
    response: &str,
    router_case: Option<RouterCase>,
    alternate_cooling_down: bool,
) -> (RetryDecision, usize, bool) {
    let through_router = router_case.is_some();
    let _lock = jcode_base::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for RestoreEnvironment {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => jcode_base::env::set_var(key, value),
                    None => jcode_base::env::remove_var(key),
                }
            }
            jcode_base::provider::models::reset_model_catalog_services_for_tests();
        }
    }
    let keys = [
        "HOME",
        "JCODE_HOME",
        "JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "JCODE_ANTHROPIC_API_KEY_NAME",
        "JCODE_ANTHROPIC_AUTH_MODE",
        "JCODE_ANTHROPIC_AUTH",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_INITIAL_PROVIDER_EXPLICIT",
        "JCODE_ANTHROPIC_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "JCODE_ANTHROPIC_API_BASE",
    ];
    let _environment = RestoreEnvironment(
        keys.into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    for key in keys {
        jcode_base::env::remove_var(key);
    }
    jcode_base::env::set_var("HOME", home.path());
    jcode_base::env::set_var("JCODE_HOME", home.path());
    jcode_base::provider::models::reset_model_catalog_services_for_tests();
    let mut config = jcode_base::config::Config::default();
    config.provider.same_provider_account_failover = enabled;
    config.save().unwrap();
    let mut account_labels = Vec::new();
    for label in if alternate {
        vec!["first", "second"]
    } else {
        vec!["first"]
    } {
        let stored_label =
            jcode_base::auth::claude::upsert_account(jcode_base::auth::claude::AnthropicAccount {
                label: label.into(),
                access: format!("access-{label}"),
                refresh: format!("refresh-{label}"),
                expires: if label == "first"
                    && router_case.is_some_and(|case| case.cached_first_rejection)
                {
                    1
                } else {
                    i64::MAX
                },
                email: None,
                subscription_type: None,
                scopes: vec![],
            })
            .unwrap();
        jcode_base::provider::clear_claude_provider_unavailability_for_account_label(Some(
            &stored_label,
        ));
        account_labels.push(stored_label);
    }
    let mut _exhaustion_target_guard = None;
    if router_case.is_some_and(|case| case.exhaust_alternate) {
        // A cross-provider prompt must name a configured, explicitly enrolled
        // billing identity. The test never applies this final target.
        jcode_base::env::set_var("OPENAI_API_KEY", "sk-test-exhaustion-target");
        let previous = jcode_base::provider::external::instantiate_external_provider(
            jcode_base::provider::external::OPENAI_RUNTIME,
        );
        let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let factory_active = active.clone();
        jcode_base::provider::external::register_external_provider_fallible(
            jcode_base::provider::external::OPENAI_RUNTIME,
            move || {
                if factory_active.load(Ordering::SeqCst) {
                    Some(Arc::new(ExhaustionApiTarget) as Arc<dyn jcode_provider_core::Provider>)
                } else {
                    previous.as_ref().map(|runtime| runtime.fork())
                }
            },
        );
        _exhaustion_target_guard = Some(ExhaustionTargetGuard(active));
        let scope = jcode_base::provider::openai_catalog_scope_for_credential(
            false,
            "sk-test-exhaustion-target",
        );
        jcode_base::provider::populate_account_models_for_scope(&scope, vec!["gpt-5.4".into()]);
        jcode_base::auth::account_pool::AccountPool {
            members: account_labels
                .iter()
                .map(|label| format!("claude:{label}"))
                .chain(std::iter::once("openai-api".into()))
                .collect(),
            excluded: Vec::new(),
            ..Default::default()
        }
        .save()
        .unwrap();
    }
    if router_case.is_some_and(|case| case.cached_first_rejection) {
        jcode_base::auth::refresh_state::record_permanent_rejection(
            "claude",
            "refresh-first",
            "invalid_grant",
        )
        .unwrap();
    }
    jcode_base::auth::claude::set_active_account(&account_labels[0]).unwrap();
    let previous_override = jcode_base::auth::claude::get_active_account_override();
    jcode_base::auth::claude::set_active_account_override(Some(account_labels[0].clone()));
    struct RestoreOverride(Option<String>);
    impl Drop for RestoreOverride {
        fn drop(&mut self) {
            jcode_base::auth::claude::set_active_account_override(self.0.take());
        }
    }
    let _restore = RestoreOverride(previous_override);
    if alternate_cooling_down {
        assert!(account_labels.len() > 1);
        jcode_base::auth::claude::set_active_account_override(Some(account_labels[1].clone()));
        jcode_base::provider::record_provider_unavailable_for_account(
            "claude",
            "rate limit reached",
        );
        jcode_base::auth::claude::set_active_account_override(Some(account_labels[0].clone()));
        assert!(!jcode_base::provider::has_eligible_alternate_account(
            "claude"
        ));
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!(
        "http://{}/v1/messages?beta=true",
        listener.local_addr().unwrap()
    );
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = requests.clone();
    let response = response.to_owned();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        // Cached rejection must never send a request for the first account.
        let first_attempt =
            usize::from(router_case.is_some_and(|case| case.cached_first_rejection));
        for attempt in first_attempt..if through_router { 2 } else { 1 } {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "expected HTTP account request did not arrive"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("local HTTP accept failed: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0, "request ended before complete body");
                request.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if request.len() >= header_end + 4 + content_length {
                        let expected_token = if through_router {
                            if attempt == 0 {
                                "access-first"
                            } else {
                                "access-second"
                            }
                        } else {
                            "test-token"
                        };
                        assert!(
                            headers
                                .to_ascii_lowercase()
                                .contains(&format!("authorization: bearer {expected_token}"))
                        );
                        break;
                    }
                }
            }
            server_requests.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 || router_case.is_some_and(|case| case.exhaust_alternate) {
                socket.write_all(response.as_bytes()).unwrap();
            } else {
                let body = concat!(
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                );
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        }
    });

    // Construct outside the runtime, avoiding unrelated background usage HTTP.
    let mut provider = AnthropicProvider::new();
    provider.client = Client::builder().no_proxy().build().unwrap();
    provider.direct_transport.oauth_api_url = Some(endpoint.clone());
    provider.oauth_preflight_done.store(true, Ordering::Relaxed);
    let retry_settings = reasoning_request::RetrySettings::from_provider(&provider);
    let mut transport = DirectTransportConfig::from_env();
    transport.oauth_api_url = Some(endpoint);
    struct RestoreRouterEnv(Option<std::ffi::OsString>, Option<std::ffi::OsString>);
    impl Drop for RestoreRouterEnv {
        fn drop(&mut self) {
            for (key, value) in [
                ("JCODE_ACTIVE_PROVIDER", self.0.take()),
                ("JCODE_ANTHROPIC_MODEL", self.1.take()),
            ] {
                match value {
                    Some(value) => jcode_base::env::set_var(key, value),
                    None => jcode_base::env::remove_var(key),
                }
            }
            jcode_base::provider::external::register_external_provider(
                jcode_base::provider::external::ANTHROPIC_RUNTIME,
                || Arc::new(AnthropicProvider::new()),
            );
        }
    }
    let mut restore_initial_provider = None;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime_guard = runtime.enter();
    let router = if through_router {
        restore_initial_provider = Some(RestoreRouterEnv(
            std::env::var_os("JCODE_ACTIVE_PROVIDER"),
            std::env::var_os("JCODE_ANTHROPIC_MODEL"),
        ));
        jcode_base::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
        jcode_base::env::set_var("JCODE_INITIAL_PROVIDER_EXPLICIT", "1");
        jcode_base::env::set_var("JCODE_ANTHROPIC_MODEL", "claude-sonnet-4-6");
        for is_oauth in [true, false] {
            let scope = jcode_base::provider::anthropic_catalog_scope_for_route(is_oauth);
            jcode_base::provider::populate_anthropic_models_for_scope(
                &scope,
                vec!["claude-sonnet-4-6".into()],
            );
        }
        let provider = Arc::new(provider);
        jcode_base::provider::external::register_external_provider(
            jcode_base::provider::external::ANTHROPIC_RUNTIME,
            move || provider.clone(),
        );
        let router = jcode_base::provider::MultiProvider::from_auth_status(
            jcode_base::auth::AuthStatus::default(),
        );
        if router_case.is_some_and(|case| case.exhaust_alternate) {
            router
                .set_model("openai-api:gpt-5.4")
                .expect("construct configured exhaustion target runtime");
        }
        router.set_model("claude:claude-sonnet-4-6").unwrap();
        if router_case.is_some_and(|case| case.exhaust_alternate) {
            assert!(
                router.model_routes().iter().any(|route| route.available
                    && route.model == "gpt-5.4"
                    && route.api_method == "openai-api-key"),
                "exhaustion fixture requires an available exact OpenAI API model"
            );
        }
        Some(router)
    } else {
        None
    };
    let (decision, rolled_back) = runtime.block_on(async {
        if let Some(router) = &router {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                let case = router_case.unwrap();
                let initial_result = if case.split {
                    router
                        .complete_split(&[], &[], "Test", "dynamic", None)
                        .await
                } else {
                    router.complete(&[], &[], "Test", None).await
                };
                let initial_error = initial_result
                    .err()
                    .expect("first account returns a pre-switch prompt");
                let initial_prompt =
                    jcode_provider_core::parse_failover_prompt_message(&initial_error.to_string())
                        .expect("exact account fallback prompt");
                assert_eq!(initial_prompt.from_provider, "claude");
                assert_eq!(initial_prompt.to_provider, "claude");
                let target_route = initial_prompt
                    .target_route
                    .expect("account-qualified target route");
                assert_eq!(
                    target_route,
                    format!("claude-oauth@{}:claude-sonnet-4-6", account_labels[1])
                );
                let initial_requests = usize::from(!case.cached_first_rejection);
                assert_eq!(
                    requests.load(Ordering::SeqCst),
                    initial_requests,
                    "no alternate HTTP request before accepting the prompt"
                );
                assert_eq!(
                    router.oauth_account_label().as_ref(),
                    Some(&account_labels[0])
                );
                assert_eq!(
                    jcode_base::auth::claude::active_account_label().as_ref(),
                    Some(&account_labels[0]),
                    "prompt must not mutate global account preference"
                );
                assert!(
                    jcode_base::provider::provider_unavailability_detail_for_account_label(
                        "claude",
                        Some(&account_labels[0])
                    )
                    .is_some()
                );
                assert!(
                    jcode_base::provider::provider_unavailability_detail_for_account_label(
                        "claude",
                        Some(&account_labels[1])
                    )
                    .is_none()
                );
                tokio::task::yield_now().await;
                assert_eq!(requests.load(Ordering::SeqCst), initial_requests);
                assert!(
                    jcode_base::provider::MultiProvider::is_failover_route_pool_approved(
                        &target_route
                    )
                );
                router
                    .set_model(&target_route)
                    .expect("explicitly accept exact account route");
                assert_eq!(
                    router.oauth_account_label().as_ref(),
                    Some(&account_labels[1])
                );
                assert_eq!(
                    jcode_base::auth::claude::active_account_label().as_ref(),
                    Some(&account_labels[0]),
                    "session-local selection must not rewrite global preference"
                );
                let result = if case.split {
                    router
                        .complete_split(&[], &[], "Test", "dynamic", None)
                        .await
                } else {
                    router.complete(&[], &[], "Test", None).await
                };
                if case.exhaust_alternate {
                    let error = result.err().expect("second account exhausted");
                    let prompt =
                        jcode_provider_core::parse_failover_prompt_message(&error.to_string())
                            .expect("configured cross-provider target after both accounts fail");
                    assert_eq!(prompt.from_provider, "claude");
                    assert_eq!(prompt.to_provider, "openai");
                    let cross_route = prompt
                        .target_route
                        .as_ref()
                        .expect("exact enrolled API target");
                    assert!(cross_route.starts_with("openai-api:"), "exhaustion target must retain explicitly enrolled API billing identity: {cross_route}");
                    assert!(
                        jcode_base::provider::MultiProvider::is_failover_route_pool_approved(
                            cross_route
                        )
                    );
                    assert_eq!(requests.load(Ordering::SeqCst), 2);
                    assert_eq!(
                        router.oauth_account_label().as_ref(),
                        Some(&account_labels[1])
                    );
                    assert_eq!(
                        jcode_base::auth::claude::active_account_label().as_ref(),
                        Some(&account_labels[0])
                    );
                    for label in &account_labels {
                        assert!(
                            jcode_base::provider::provider_unavailability_detail_for_account_label(
                                "claude",
                                Some(label)
                            )
                            .is_some(),
                            "each failed token retains its own cooldown"
                        );
                    }
                    assert!(!jcode_base::provider::has_eligible_alternate_account(
                        "claude"
                    ));
                    let next_error = if case.split {
                        router
                            .complete_split(&[], &[], "Test", "dynamic", None)
                            .await
                    } else {
                        router.complete(&[], &[], "Test", None).await
                    }
                    .err()
                    .expect("cached exhaustion must not make another HTTP request");
                    let next_prompt =
                        jcode_provider_core::parse_failover_prompt_message(&next_error.to_string())
                            .expect("cached exhaustion retains exact cross-provider prompt");
                    assert_eq!(next_prompt.target_route, prompt.target_route);
                    assert_eq!(requests.load(Ordering::SeqCst), 2);
                    return (RetryDecision::Yield(prompt.reason), false);
                }
                let mut stream = result.expect("explicitly selected second account must succeed");
                while let Some(event) = stream.next().await {
                    let event = event.expect("accepted stream must not expose first account error");
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        assert_eq!(
                            router.oauth_account_label().as_ref(),
                            Some(&account_labels[1]),
                            "accepted alternate remains session-bound"
                        );
                        assert_eq!(
                            jcode_base::auth::claude::active_account_label().as_ref(),
                            Some(&account_labels[0]),
                            "accepted alternate never changes another session's preference"
                        );
                        assert_eq!(requests.load(Ordering::SeqCst), initial_requests + 1);
                        return (RetryDecision::Complete, false);
                    }
                }
                panic!("working account stream must end with MessageEnd");
            })
            .await
            .expect("router must switch accounts without 60s sleep")
        } else {
            let (tx, mut rx) = mpsc::channel(64);
            let task = tokio::spawn(run_stream_with_retries(
                Client::builder().no_proxy().build().unwrap(),
                "test-token".into(),
                true,
                ApiRequest {
                    model: "claude-sonnet-4-6".into(),
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
                },
                tx,
                Arc::new(RwLock::new(None)),
                Some(account_labels[0].clone()),
                "claude-sonnet-4-6".into(),
                "test-session".into(),
                Arc::new(std::sync::RwLock::new("claude-sonnet-4-6".into())),
                transport,
                retry_settings,
            ));
            let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                let mut rolled_back = false;
                while let Some(event) = rx.recv().await {
                    match event {
                        Err(error) => {
                            return (RetryDecision::Yield(error.to_string()), rolled_back);
                        }
                        Ok(StreamEvent::RetryRollback { .. }) => rolled_back = true,
                        Ok(StreamEvent::ConnectionPhase {
                            phase: ConnectionPhase::Retrying { .. },
                        }) => {
                            return (RetryDecision::Retry, rolled_back);
                        }
                        _ => {}
                    }
                }
                panic!("retry loop ended without error or retry decision");
            })
            .await;
            task.abort();
            let _ = task.await;
            result.expect("account decision must not wait for Retry-After")
        }
    });
    drop(restore_initial_provider);
    server.join().unwrap();
    (decision, requests.load(Ordering::SeqCst), rolled_back)
}

fn rate_limit_http_response() -> String {
    let body = r#"{"error":{"type":"rate_limit_error","message":"Rate limit reached"}}"#;
    format!(
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 60\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn oauth_account_failover_yields_http_429_before_sixty_second_retry() {
    let (decision, requests, rolled_back) =
        oauth_http_retry_decision(true, true, &rate_limit_http_response());
    assert!(
        matches!(decision, RetryDecision::Yield(ref error) if error.contains("429 Too Many Requests"))
    );
    assert_eq!(requests, 1);
    assert!(!rolled_back);
}

#[test]
fn oauth_account_failover_router_switches_http_429_to_second_account_200() {
    for split in [false, true] {
        let (decision, requests, rolled_back) = http_retry_decision(
            true,
            true,
            &rate_limit_http_response(),
            Some(RouterCase {
                split,
                exhaust_alternate: false,
                cached_first_rejection: false,
            }),
            false,
        );
        assert_eq!(decision, RetryDecision::Complete);
        assert_eq!(
            requests, 2,
            "first and second accounts each receive one request"
        );
        assert!(!rolled_back);
    }
}

#[test]
fn oauth_account_failover_router_recovers_cached_refresh_rejection() {
    for split in [false, true] {
        let (decision, requests, rolled_back) = http_retry_decision(
            true,
            true,
            &rate_limit_http_response(),
            Some(RouterCase {
                split,
                exhaust_alternate: false,
                cached_first_rejection: true,
            }),
            false,
        );
        assert_eq!(decision, RetryDecision::Complete);
        assert_eq!(requests, 1, "only the healthy alternate may reach HTTP");
        assert!(!rolled_back);
    }
}

#[test]
fn oauth_account_failover_router_reports_exhaustion_after_distinct_tokens_each_receive_429() {
    for split in [false, true] {
        let (decision, requests, rolled_back) = http_retry_decision(
            true,
            true,
            &rate_limit_http_response(),
            Some(RouterCase {
                split,
                exhaust_alternate: true,
                cached_first_rejection: false,
            }),
            false,
        );
        assert!(matches!(decision, RetryDecision::Yield(_)));
        assert_eq!(
            requests, 2,
            "each distinct OAuth token must be attempted exactly once"
        );
        assert!(!rolled_back);
    }
}

#[test]
fn oauth_account_failover_yields_http_429_when_alternate_is_cooling_down() {
    let (decision, requests, _) =
        http_retry_decision(true, true, &rate_limit_http_response(), None, true);
    assert!(
        matches!(decision, RetryDecision::Yield(ref error) if error.contains("429 Too Many Requests"))
    );
    assert_eq!(requests, 1);
}

#[test]
fn oauth_account_failover_disabled_keeps_http_rate_limit_retry() {
    let (decision, requests, _) =
        oauth_http_retry_decision(false, true, &rate_limit_http_response());
    assert_eq!(decision, RetryDecision::Retry);
    assert_eq!(requests, 1);
}

#[test]
fn oauth_account_failover_single_account_keeps_http_rate_limit_retry() {
    let (decision, requests, _) =
        oauth_http_retry_decision(true, false, &rate_limit_http_response());
    assert_eq!(decision, RetryDecision::Retry);
    assert_eq!(requests, 1);
}

#[test]
fn oauth_account_failover_preserves_transport_server_retry() {
    let response = "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 60\r\nContent-Length: 10\r\nConnection: close\r\n\r\noverloaded";
    let (decision, requests, _) = oauth_http_retry_decision(true, true, response);
    assert_eq!(decision, RetryDecision::Retry);
    assert_eq!(requests, 1);
}

#[test]
fn oauth_account_failover_does_not_rotate_after_partial_output() {
    let body = concat!(
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"Rate limit reached\"}}\n\n",
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (decision, requests, rolled_back) = oauth_http_retry_decision(true, true, &response);
    assert_eq!(decision, RetryDecision::Retry);
    assert_eq!(requests, 1);
    assert!(rolled_back, "partial output must retain the rollback path");
}

#[test]
fn oauth_account_failover_preserves_retry_for_accepted_http_early_sse_quota() {
    let body = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"Rate limit reached\"}}\n\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (decision, requests, rolled_back) = oauth_http_retry_decision(true, true, &response);
    assert_eq!(decision, RetryDecision::Retry);
    assert_eq!(requests, 1);
    assert!(!rolled_back);
}
