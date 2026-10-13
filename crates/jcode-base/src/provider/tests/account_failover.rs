#[derive(Clone)]
struct AccountRoutingStreamProvider {
    attempts: Arc<Mutex<Vec<String>>>,
    failures: HashMap<String, String>,
}

#[async_trait::async_trait]
impl Provider for AccountRoutingStreamProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        use crate::message::{ConnectionPhase, StreamEvent};
        let label = auth::codex::active_account_label().expect("active test login");
        self.attempts.lock().unwrap().push(label.clone());
        let mut events = vec![
            Ok(StreamEvent::ConnectionType {
                connection: "https/sse".into(),
            }),
            Ok(StreamEvent::ConnectionPhase {
                phase: ConnectionPhase::SendingRequest,
            }),
        ];
        match self.failures.get(&label) {
            Some(error) if error.starts_with("event:") => events.push(Ok(StreamEvent::Error {
                message: error.trim_start_matches("event:").into(),
                retry_after_secs: Some(60),
            })),
            Some(error) => events.push(Err(anyhow!("{error}"))),
            None => {
                events.push(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::WaitingForResponse,
                }));
                events.push(Ok(StreamEvent::TextDelta(format!("served by {label}"))));
                events.push(Ok(StreamEvent::MessageEnd { stop_reason: None }));
            }
        }
        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "OpenAI"
    }
    fn model(&self) -> String {
        "gpt-5.4".into()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct AccountRoutingFixtureCleanup;

impl Drop for AccountRoutingFixtureCleanup {
    fn drop(&mut self) {
        for label in ["openai-otter", "openai-fox", "openai-panda", "openai-wolf"] {
            super::models::clear_openai_provider_unavailability_for_account_label(Some(label));
        }
    }
}

fn account_routing_fixture(failures: &[(&str, &str)]) -> (MultiProvider, Arc<Mutex<Vec<String>>>, AccountRoutingFixtureCleanup) {
    let provider = test_multi_provider_with_openai();
    for label in ["openai-otter", "openai-fox", "openai-panda", "openai-wolf"] {
        auth::codex::upsert_account_from_tokens(label, "fake-access", "fake-refresh", None, None)
            .expect("save fixture login");
        auth::codex::set_active_account_override(Some(label.into()));
        clear_provider_unavailable_for_account("openai");
    }
    let labels = ["openai-otter", "openai-fox", "openai-panda"];
    crate::auth::account_pool::AccountPool {
        members: labels
            .iter()
            .map(|label| format!("openai:{label}"))
            .collect(),
        excluded: auth::codex::list_accounts()
            .unwrap()
            .into_iter()
            .filter(|account| !labels.contains(&account.label.as_str()))
            .map(|account| format!("openai:{}", account.label))
            .collect(),
    }
    .save()
    .expect("save fixture pool");
    auth::codex::set_active_account_override(Some("openai-otter".into()));
    let attempts = Arc::new(Mutex::new(Vec::new()));
    *provider.openai.write().unwrap() = Some(Arc::new(AccountRoutingStreamProvider {
        attempts: Arc::clone(&attempts),
        failures: failures
            .iter()
            .map(|(label, error)| ((*label).into(), (*error).into()))
            .collect(),
    }));
    (provider, attempts, AccountRoutingFixtureCleanup)
}

#[test]
fn account_router_rotates_through_deferred_stream_errors_in_both_completion_modes() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            use futures::StreamExt;
            for split in [false, true] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[
                    ("openai-otter", "429 Too Many Requests"),
                    ("openai-fox", "event:rate limit exceeded"),
                ]);
                let mut stream = if split {
                    provider
                        .complete_split(&[], &[], "static", "dynamic", None)
                        .await
                } else {
                    provider.complete(&[], &[], "system", None).await
                }
                .expect("rotate to usable login");
                let mut text = String::new();
                while let Some(event) = stream.next().await {
                    if let crate::message::StreamEvent::TextDelta(delta) = event.unwrap() {
                        text.push_str(&delta);
                    }
                }
                assert_eq!(text, "served by openai-panda");
                assert_eq!(
                    *attempts.lock().unwrap(),
                    ["openai-otter", "openai-fox", "openai-panda"]
                );
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-panda")
                );
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-otter")
                    )
                    .is_some()
                );
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-fox")
                    )
                    .is_some()
                );
                assert!(provider_unavailability_detail_for_account("openai").is_none());
            }
        });
    });
}

#[test]
fn account_router_rotates_cached_unavailability_without_retrying_spent_or_excluded_accounts() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            record_provider_unavailable_for_account("openai", "active quota exhausted");
            auth::codex::set_active_account_override(Some("openai-fox".into()));
            record_provider_unavailable_for_account("openai", "alternate quota exhausted");
            auth::codex::set_active_account_override(Some("openai-otter".into()));
            assert!(has_eligible_alternate_account("openai"));
            assert_eq!(
                same_provider_account_candidates(ActiveProvider::OpenAI),
                ["openai-panda"]
            );
            let _stream = provider
                .complete(&[], &[], "system", None)
                .await
                .expect("rotate cached error");
            assert_eq!(*attempts.lock().unwrap(), ["openai-panda"]);
        });
    });
}

#[test]
fn account_router_exhaustion_restores_original_login_and_requests_cross_provider_consent() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[
                ("openai-otter", "429 Too Many Requests"),
                ("openai-fox", "429 Too Many Requests"),
                ("openai-panda", "429 Too Many Requests"),
            ]);
            let err = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("all logins exhausted");
            assert!(parse_failover_prompt_message(&err.to_string()).is_some());
            assert_eq!(
                *attempts.lock().unwrap(),
                ["openai-otter", "openai-fox", "openai-panda"]
            );
            assert_eq!(
                auth::codex::active_account_label().as_deref(),
                Some("openai-otter")
            );
            assert!(!has_eligible_alternate_account("openai"));
            let err = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("cooldowns retained");
            assert!(parse_failover_prompt_message(&err.to_string()).is_some());
            assert_eq!(
                attempts.lock().unwrap().len(),
                3,
                "do not revisit spent accounts next turn"
            );
        });
    });
}

#[test]
fn account_router_does_not_hide_non_account_errors_from_alternate_login() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[
                ("openai-otter", "429 Too Many Requests"),
                ("openai-fox", "400 invalid tool schema"),
            ]);
            let err = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("invalid request");
            assert_eq!(err.to_string(), "400 invalid tool schema");
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter", "openai-fox"]);
            assert_eq!(
                auth::codex::active_account_label().as_deref(),
                Some("openai-otter")
            );
        });
    });
}

#[test]
fn account_router_keeps_context_errors_out_of_account_rotation() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) =
                account_routing_fixture(&[("openai-otter", "context length exceeded")]);
            let err = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("model context failure");
            assert!(parse_failover_prompt_message(&err.to_string()).is_some());
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            assert!(provider_unavailability_detail_for_account("openai").is_none());
        });
    });
}

#[test]
fn account_router_respects_disabled_failover() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            crate::env::set_var("JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER", "false");
            let (provider, attempts, _cleanup) =
                account_routing_fixture(&[("openai-otter", "429 Too Many Requests")]);
            assert!(!has_eligible_alternate_account("openai"));
            let _stream = provider
                .complete(&[], &[], "system", None)
                .await
                .expect("unwrapped stream when disabled");
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            assert_eq!(
                auth::codex::active_account_label().as_deref(),
                Some("openai-otter")
            );
        });
    });
}

#[derive(Clone)]
struct PendingAccountRoutingProvider {
    attempts: Arc<Mutex<Vec<String>>>,
    cached_login: Arc<Mutex<String>>,
    reloads: Arc<Mutex<Vec<String>>>,
    concurrent_override: Option<String>,
}

#[async_trait::async_trait]
impl Provider for PendingAccountRoutingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let label = auth::codex::active_account_label().unwrap();
        assert_eq!(*self.cached_login.lock().unwrap(), label);
        self.attempts.lock().unwrap().push(label.clone());
        if label == "openai-otter" {
            return Ok(Box::pin(futures::stream::iter(vec![Err(anyhow!(
                "429 Too Many Requests"
            ))])));
        }
        if let Some(label) = &self.concurrent_override {
            auth::codex::set_active_account_override(Some(label.clone()));
        }
        // No readiness metadata: the router must still own this request when
        // the timeout cancels the completion future.
        Ok(Box::pin(futures::stream::pending()))
    }

    async fn invalidate_credentials(&self) {
        *self.cached_login.lock().unwrap() = auth::codex::active_account_label().unwrap();
    }

    fn reload_credentials(&self) {
        let label = auth::codex::active_account_label().unwrap();
        *self.cached_login.lock().unwrap() = label.clone();
        self.reloads.lock().unwrap().push(label);
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn model(&self) -> String {
        "gpt-5.4".into()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        // Mirror OpenAI's shared credential cache to verify that merely forking
        // is not sufficient to restore the request's original login.
        Arc::new(self.clone())
    }
}

#[test]
fn account_router_cancellation_restores_exact_override_and_runtime_credentials() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for split in [false, true] {
                for prior_override in [None, Some("openai-otter".to_owned())] {
                    let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                    auth::codex::set_active_account("openai-otter").unwrap();
                    auth::codex::set_active_account_override(prior_override.clone());
                    let cached_login = Arc::new(Mutex::new("openai-otter".to_owned()));
                    let reloads = Arc::new(Mutex::new(Vec::new()));
                    let runtime: Arc<dyn Provider> = Arc::new(PendingAccountRoutingProvider {
                        attempts: attempts.clone(),
                        cached_login: cached_login.clone(),
                        reloads: reloads.clone(),
                        concurrent_override: None,
                    });
                    *provider.openai.write().unwrap() = Some(runtime.clone());
                    let result =
                        tokio::time::timeout(std::time::Duration::from_millis(100), async {
                            if split {
                                provider
                                    .complete_split(&[], &[], "static", "dynamic", None)
                                    .await
                            } else {
                                provider.complete(&[], &[], "system", None).await
                            }
                        })
                        .await;
                    assert!(result.is_err(), "alternate startup must still be pending");
                    assert_eq!(*attempts.lock().unwrap(), ["openai-otter", "openai-fox"]);
                    assert_eq!(auth::codex::get_active_account_override(), prior_override);
                    assert_eq!(*cached_login.lock().unwrap(), "openai-otter");
                    assert_eq!(*reloads.lock().unwrap(), ["openai-otter"]);
                    assert!(!Arc::ptr_eq(
                        provider.openai.read().unwrap().as_ref().unwrap(),
                        &runtime
                    ));
                    assert!(
                        super::models::provider_unavailability_detail_for_account_label(
                            "openai",
                            Some("openai-otter")
                        )
                        .is_some()
                    );
                    assert!(
                        super::models::provider_unavailability_detail_for_account_label(
                            "openai",
                            Some("openai-fox")
                        )
                        .is_none()
                    );
                }
            }
        });
    });
}

#[test]
fn account_router_cancellation_does_not_overwrite_another_account_selection() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            let reloads = Arc::new(Mutex::new(Vec::new()));
            let runtime: Arc<dyn Provider> = Arc::new(PendingAccountRoutingProvider {
                attempts: attempts.clone(),
                cached_login: Arc::new(Mutex::new("openai-otter".into())),
                reloads: reloads.clone(),
                concurrent_override: Some("openai-panda".into()),
            });
            *provider.openai.write().unwrap() = Some(runtime.clone());
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                provider.complete(&[], &[], "system", None),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter", "openai-fox"]);
            assert_eq!(
                auth::codex::get_active_account_override().as_deref(),
                Some("openai-panda")
            );
            assert!(reloads.lock().unwrap().is_empty());
            assert!(Arc::ptr_eq(
                provider.openai.read().unwrap().as_ref().unwrap(),
                &runtime
            ));
        });
    });
}

#[derive(Clone)]
struct ApiKeyAccountRoutingProvider {
    inner: AccountRoutingStreamProvider,
    mode: CredentialMode,
    synchronous_limit: bool,
}

#[async_trait::async_trait]
impl Provider for ApiKeyAccountRoutingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let stream = self
            .inner
            .complete(messages, tools, system, resume_session_id)
            .await?;
        if self.synchronous_limit {
            return Err(anyhow!("429 Too Many Requests"));
        }
        Ok(stream)
    }

    fn credential_mode(&self) -> CredentialMode {
        self.mode
    }

    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(jcode_provider_core::ResolvedCredential::ApiKey)
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn model(&self) -> String {
        "gpt-5.4".into()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn account_router_api_key_requests_ignore_and_preserve_oauth_cooldown() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            use futures::StreamExt;
            for mode in [CredentialMode::ApiKey, CredentialMode::Auto] {
                for split in [false, true] {
                    let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                    record_provider_unavailable_for_account(
                        "openai",
                        "saved OAuth quota exhausted",
                    );
                    *provider.openai.write().unwrap() =
                        Some(Arc::new(ApiKeyAccountRoutingProvider {
                            inner: AccountRoutingStreamProvider {
                                attempts: attempts.clone(),
                                failures: HashMap::new(),
                            },
                            mode,
                            synchronous_limit: false,
                        }));
                    let mut stream = if split {
                        provider
                            .complete_split(&[], &[], "static", "dynamic", None)
                            .await
                    } else {
                        provider.complete(&[], &[], "system", None).await
                    }
                    .expect("OAuth cooldown must not block an API-key request");
                    let mut text = String::new();
                    while let Some(event) = stream.next().await {
                        if let crate::message::StreamEvent::TextDelta(delta) = event.unwrap() {
                            text.push_str(&delta);
                        }
                    }
                    assert_eq!(text, "served by openai-otter");
                    assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
                    assert_eq!(
                        auth::codex::active_account_label().as_deref(),
                        Some("openai-otter")
                    );
                    assert!(
                        provider_unavailability_detail_for_account("openai")
                            .unwrap()
                            .contains("saved OAuth quota exhausted")
                    );
                }
            }
        });
    });
}

#[test]
fn account_router_api_key_limit_does_not_rotate_or_poison_oauth_accounts() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for mode in [CredentialMode::ApiKey, CredentialMode::Auto] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                *provider.openai.write().unwrap() = Some(Arc::new(ApiKeyAccountRoutingProvider {
                    inner: AccountRoutingStreamProvider {
                        attempts: attempts.clone(),
                        failures: HashMap::new(),
                    },
                    mode,
                    synchronous_limit: true,
                }));
                let error = provider
                    .complete(&[], &[], "system", None)
                    .await
                    .err()
                    .expect("API key is limited");
                assert!(parse_failover_prompt_message(&error.to_string()).is_some());
                assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-otter")
                );
                for label in ["openai-otter", "openai-fox", "openai-panda"] {
                    assert!(
                        super::models::provider_unavailability_detail_for_account_label(
                            "openai",
                            Some(label)
                        )
                        .is_none()
                    );
                }
            }
        });
    });
}

#[derive(Clone)]
struct AlternateAutoApiKeyProvider {
    inner: AccountRoutingStreamProvider,
    resolved_api_key: Arc<std::sync::atomic::AtomicBool>,
    fail_alternate: bool,
}

#[async_trait::async_trait]
impl Provider for AlternateAutoApiKeyProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let stream = self
            .inner
            .complete(messages, tools, system, resume_session_id)
            .await?;
        if auth::codex::active_account_label().as_deref() == Some("openai-fox") {
            self.resolved_api_key
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if self.fail_alternate {
                return Err(anyhow!("429 Too Many Requests"));
            }
        }
        Ok(stream)
    }

    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(
            if self
                .resolved_api_key
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                jcode_provider_core::ResolvedCredential::ApiKey
            } else {
                jcode_provider_core::ResolvedCredential::Oauth
            },
        )
    }

    fn reload_credentials(&self) {
        self.resolved_api_key
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn model(&self) -> String {
        "gpt-5.4".into()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn account_router_alternate_auto_api_fallback_restores_override_without_poisoning_pool() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for fail_alternate in [false, true] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                let notices_before = provider.startup_notices.read().unwrap().len();
                *provider.openai.write().unwrap() = Some(Arc::new(AlternateAutoApiKeyProvider {
                    inner: AccountRoutingStreamProvider {
                        attempts: attempts.clone(),
                        failures: [("openai-otter".into(), "429 Too Many Requests".into())].into(),
                    },
                    resolved_api_key: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    fail_alternate,
                }));
                let result = provider.complete(&[], &[], "system", None).await;
                if fail_alternate {
                    assert_eq!(
                        result.err().expect("API key is limited").to_string(),
                        "429 Too Many Requests"
                    );
                } else {
                    assert!(
                        result.is_ok(),
                        "Auto's API-key fallback may serve the request"
                    );
                }
                assert_eq!(*attempts.lock().unwrap(), ["openai-otter", "openai-fox"]);
                assert_eq!(
                    auth::codex::get_active_account_override().as_deref(),
                    Some("openai-otter")
                );
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-otter")
                    )
                    .is_some()
                );
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-fox")
                    )
                    .is_none()
                );
                assert_eq!(
                    provider.startup_notices.read().unwrap().len(),
                    notices_before
                );
            }
        });
    });
}
