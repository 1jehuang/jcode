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
    fn credential_mode(&self) -> CredentialMode {
        CredentialMode::OAuth
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct AccountRoutingFixtureCleanup;

impl Drop for AccountRoutingFixtureCleanup {
    fn drop(&mut self) {
        clear_provider_unavailable_for_account("openai-api");
        clear_provider_unavailable_for_account("anthropic-api");
        for label in ["openai-otter", "openai-fox", "openai-panda", "openai-wolf"] {
            super::models::clear_openai_provider_unavailability_for_account_label(Some(label));
        }
    }
}

fn account_routing_fixture(
    failures: &[(&str, &str)],
) -> (
    MultiProvider,
    Arc<Mutex<Vec<String>>>,
    AccountRoutingFixtureCleanup,
) {
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
fn account_router_does_not_rotate_invalid_requests() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) =
                account_routing_fixture(&[("openai-otter", "400 invalid encrypted_content")]);
            // The per-window account failover replays errors that are not
            // account-local inside the stream, untouched.
            let error = match provider.complete(&[], &[], "system", None).await {
                Err(error) => error,
                Ok(mut stream) => {
                    use futures::StreamExt;
                    let mut first_error = None;
                    while let Some(item) = stream.next().await {
                        if let Err(error) = item {
                            first_error = Some(error);
                            break;
                        }
                    }
                    first_error.expect("invalid request error")
                }
            };
            assert_eq!(error.to_string(), "400 invalid encrypted_content");
            assert!(parse_failover_prompt_message(&error.to_string()).is_none());
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
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("surface account error when no cross-provider pool route exists");
            assert!(parse_failover_prompt_message(&error.to_string()).is_none());
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
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
            for mode in [CredentialMode::ApiKey] {
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
            for mode in [CredentialMode::ApiKey] {
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
                assert!(
                    parse_failover_prompt_message(&error.to_string()).is_none(),
                    "excluded API source must not initiate automatic pool routing"
                );
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

struct BoundAccountRoutingProvider {
    attempts: Arc<Mutex<Vec<String>>>,
    failures: HashMap<String, String>,
    label: Mutex<String>,
    mode: Mutex<CredentialMode>,
}

#[async_trait::async_trait]
impl Provider for BoundAccountRoutingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> Result<EventStream> {
        assert_eq!(
            *self.mode.lock().unwrap(),
            CredentialMode::OAuth,
            "Auto would spend API credits after OAuth failure"
        );
        let label = self.label.lock().unwrap().clone();
        self.attempts.lock().unwrap().push(label.clone());
        let event = match self.failures.get(&label) {
            Some(error) => Err(anyhow!("{error}")),
            None => Ok(crate::message::StreamEvent::TextDelta(format!(
                "served by {label}"
            ))),
        };
        Ok(Box::pin(futures::stream::iter(vec![event])))
    }
    fn name(&self) -> &str {
        "OpenAI"
    }
    fn model(&self) -> String {
        "gpt-5.4".into()
    }
    fn credential_mode(&self) -> CredentialMode {
        *self.mode.lock().unwrap()
    }
    fn oauth_account_label(&self) -> Option<String> {
        Some(self.label.lock().unwrap().clone())
    }
    fn context_window(&self) -> usize {
        1024
    }
    fn set_oauth_account(&self, label: Option<&str>) -> Result<()> {
        *self.label.lock().unwrap() = label.unwrap().to_owned();
        Ok(())
    }
    fn set_credential_mode(&self, mode: CredentialMode) -> Result<()> {
        *self.mode.lock().unwrap() = mode;
        Ok(())
    }
    fn set_model(&self, _model: &str) -> Result<()> {
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            attempts: self.attempts.clone(),
            failures: self.failures.clone(),
            label: Mutex::new(self.label.lock().unwrap().clone()),
            mode: Mutex::new(*self.mode.lock().unwrap()),
        })
    }
}

fn bind_fixture_account(
    provider: &MultiProvider,
    attempts: &Arc<Mutex<Vec<String>>>,
    label: &str,
    failures: &[(&str, &str)],
) {
    *provider.openai.write().unwrap() = Some(Arc::new(BoundAccountRoutingProvider {
        attempts: attempts.clone(),
        failures: failures
            .iter()
            .map(|(label, error)| ((*label).into(), (*error).into()))
            .collect(),
        label: Mutex::new(label.into()),
        mode: Mutex::new(CredentialMode::OAuth),
    }));
}

#[test]
fn pinned_complete_can_cross_pool_boundary_for_context_or_disabled_same_provider() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            crate::env::set_var("ANTHROPIC_API_KEY", "sk-test-cross-pool");
            // With same-provider failover on, the per-window account engine
            // owns the request (the agent compacts on context limits), so the
            // cross-pool prompt is exercised with it off.
            for (same_provider_enabled, failure) in [(false, "429 quota exhausted")] {
                crate::env::set_var(
                    "JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER",
                    if same_provider_enabled {
                        "true"
                    } else {
                        "false"
                    },
                );
                for split in [false, true] {
                    let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                    bind_fixture_account(
                        &provider,
                        &attempts,
                        "openai-otter",
                        &[("openai-otter", failure)],
                    );
                    let mut pool = crate::auth::account_pool::AccountPool::load();
                    pool.members.insert(1, "anthropic-api".into());
                    pool.save().unwrap();
                    let result = if split {
                        provider
                            .complete_split(&[], &[], "static", "dynamic", None)
                            .await
                    } else {
                        provider.complete(&[], &[], "system", None).await
                    };
                    let error = result.err().expect("pre-switch cross-provider prompt");
                    let prompt = parse_failover_prompt_message(&error.to_string())
                        .expect("typed exact route prompt");
                    assert_eq!(prompt.to_provider, "claude");
                    let route = prompt.target_route.expect("exact API route");
                    assert!(route.starts_with("claude-api:"), "{route}");
                    assert!(MultiProvider::is_failover_route_pool_approved(&route));
                    assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
                    assert_eq!(
                        provider.oauth_account_label().as_deref(),
                        Some("openai-otter")
                    );
                    assert_eq!(
                        auth::codex::active_account_label().as_deref(),
                        Some("openai-otter")
                    );
                    assert_eq!(
                        super::models::provider_unavailability_detail_for_account_label(
                            "openai",
                            Some("openai-otter")
                        )
                        .is_some(),
                        !same_provider_enabled,
                        "context-only failure must not poison account cooldown"
                    );
                    if same_provider_enabled {
                        let model = route.split_once(':').unwrap().1;
                        assert!(
                            super::models::context_limit_for_model_with_provider(
                                model,
                                Some("claude")
                            )
                            .unwrap()
                                > 1024
                        );
                    }
                }
            }
        });
    });
}

#[test]
fn exact_target_readiness_rejects_dead_refresh_and_unavailable_model() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (_provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        let route = "openai-oauth@openai-fox:gpt-5.4";
        auth::codex::upsert_account_from_tokens("openai-fox", "expired", "", None, Some(1))
            .unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        auth::codex::upsert_account_from_tokens(
            "openai-fox",
            "expired",
            "dead-refresh-fox",
            None,
            Some(1),
        )
        .unwrap();
        crate::auth::refresh_state::record_permanent_rejection(
            "openai",
            "dead-refresh-fox",
            "invalid_grant",
        )
        .unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        auth::codex::upsert_account_from_tokens(
            "openai-fox",
            "fresh",
            "new-refresh-fox",
            None,
            None,
        )
        .unwrap();
        assert!(MultiProvider::is_failover_route_pool_approved(route));
        super::models::record_model_unavailable_for_scope(
            "openai-fox",
            "gpt-5.4",
            "model access denied",
        );
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        super::models::clear_model_unavailable_for_scope("openai-fox", "gpt-5.4");
        assert!(!MultiProvider::is_failover_route_pool_approved(
            "openai-oauth@openai-fox:not-a-model"
        ));
    });
}

#[test]
fn context_failover_never_offers_known_too_small_routes() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        assert!(
            provider
                .cross_provider_pool_prompt(
                    ActiveProvider::OpenAI,
                    ActiveProvider::Claude,
                    "context length exceeded".into(),
                    0,
                    usize::MAX
                )
                .is_none()
        );
    });
}

#[test]
fn missing_or_excluded_pins_never_fallback() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for label in ["missing", "openai-wolf"] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                bind_fixture_account(
                    &provider,
                    &attempts,
                    label,
                    &[(label, "429 quota exhausted")],
                );
                let error = provider
                    .complete(&[], &[], "system", None)
                    .await
                    .err()
                    .unwrap();
                assert!(parse_failover_prompt_message(&error.to_string()).is_none());
                assert_eq!(
                    attempts.lock().unwrap().len(),
                    usize::from(label != "missing")
                );
                assert_eq!(provider.oauth_account_label().as_deref(), Some(label));
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-otter")
                );
            }
        });
    });
}

#[test]
fn timer_target_revalidation_rejects_deleted_excluded_and_cooled_accounts() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (_provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        let route = "openai-oauth@openai-fox:gpt-5.4";
        assert!(MultiProvider::is_failover_route_pool_approved(route));
        let mut pool = crate::auth::account_pool::AccountPool::load();
        pool.members.retain(|member| member != "openai:openai-fox");
        pool.excluded.push("openai:openai-fox".into());
        pool.save().unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        pool.excluded.retain(|member| member != "openai:openai-fox");
        pool.save().unwrap();
        super::models::record_provider_unavailable_for_account_label(
            "openai",
            Some("openai-fox"),
            "429 quota exhausted",
        );
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        super::models::clear_openai_provider_unavailability_for_account_label(Some("openai-fox"));
        auth::codex::remove_account("openai-fox").unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        assert!(!MultiProvider::is_failover_route_pool_approved(
            "openai-oauth:gpt-5.4"
        ));
        assert!(!MultiProvider::is_failover_route_pool_approved(
            "openai-api:gpt-5.4"
        ));
    });
}
