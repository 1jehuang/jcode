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
        ..Default::default()
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
fn account_router_prompts_before_any_alternate_request_in_both_completion_modes() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for split in [false, true] {
                let (provider, attempts, _cleanup) =
                    account_routing_fixture(&[("openai-otter", "429 Too Many Requests")]);
                let runtime = provider.openai_provider().unwrap();
                let result = if split {
                    provider
                        .complete_split(&[], &[], "static", "dynamic", None)
                        .await
                } else {
                    provider.complete(&[], &[], "system", None).await
                };
                let error = result.err().expect("return pre-switch prompt");
                let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
                assert_eq!(
                    prompt.target_route.as_deref(),
                    Some("openai-oauth@openai-fox:gpt-5.4")
                );
                assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-otter")
                );
                assert!(Arc::ptr_eq(&runtime, &provider.openai_provider().unwrap()));
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
        });
    });
}

#[test]
fn account_router_cached_failure_skips_spent_and_excluded_accounts_without_requests() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            super::models::record_provider_unavailable_for_account_label(
                "openai",
                Some("openai-otter"),
                "quota exhausted",
            );
            super::models::record_provider_unavailable_for_account_label(
                "openai",
                Some("openai-fox"),
                "quota exhausted",
            );
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-panda:gpt-5.4")
            );
            assert!(attempts.lock().unwrap().is_empty());
            assert!(!MultiProvider::is_failover_route_pool_approved(
                "openai-oauth@openai-wolf:gpt-5.4"
            ));
            assert!(!MultiProvider::is_failover_route_pool_approved(
                "openai-oauth@openai-fox:gpt-5.4"
            ));
        });
    });
}

#[test]
fn account_router_does_not_rotate_invalid_requests() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) =
                account_routing_fixture(&[("openai-otter", "400 invalid encrypted_content")]);
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
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
            for (same_provider_enabled, failure) in [
                (true, "context length exceeded"),
                (false, "429 quota exhausted"),
            ] {
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
fn auto_subscription_failure_is_pinned_before_any_api_spending() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            let mut pool = crate::auth::account_pool::AccountPool::load();
            pool.excluded.push("openai-api".into());
            pool.save().unwrap();
            let original = Arc::new(BoundAccountRoutingProvider {
                attempts: attempts.clone(),
                failures: [("openai-otter".into(), "401 OAuth refresh rejected".into())].into(),
                label: Mutex::new("unbound".into()),
                mode: Mutex::new(CredentialMode::Auto),
            });
            *provider.openai.write().unwrap() = Some(original.clone());
            let global_before = auth::codex::active_account_label();
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-fox:gpt-5.4")
            );
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            assert_eq!(
                provider.oauth_account_label().as_deref(),
                Some("openai-otter")
            );
            assert_eq!(
                provider.openai_provider().unwrap().credential_mode(),
                CredentialMode::OAuth
            );
            assert_eq!(original.credential_mode(), CredentialMode::Auto);
            assert_eq!(original.oauth_account_label().as_deref(), Some("unbound"));
            assert_eq!(auth::codex::active_account_label(), global_before);
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
fn pinned_account_prompts_use_session_identity_and_apply_without_global_override() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            use futures::StreamExt;
            for split in [false, true] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                bind_fixture_account(
                    &provider,
                    &attempts,
                    "openai-fox",
                    &[("openai-fox", "429 quota exhausted")],
                );
                let sibling = provider.fork();
                let result = if split {
                    provider
                        .complete_split(&[], &[], "static", "dynamic", None)
                        .await
                } else {
                    provider.complete(&[], &[], "system", None).await
                };
                let error = result.err().unwrap();
                let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
                let route = prompt.target_route.unwrap();
                assert_eq!(route, "openai-oauth@openai-panda:gpt-5.4");
                assert_eq!(*attempts.lock().unwrap(), ["openai-fox"]);
                assert_eq!(
                    provider.oauth_account_label().as_deref(),
                    Some("openai-fox")
                );
                assert_eq!(sibling.oauth_account_label().as_deref(), Some("openai-fox"));
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-fox")
                    )
                    .is_some()
                );
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-otter")
                    )
                    .is_none()
                );
                assert!(MultiProvider::is_failover_route_pool_approved(&route));
                provider.set_model(&route).unwrap();
                let mut stream = provider.complete(&[], &[], "system", None).await.unwrap();
                assert!(stream.next().await.unwrap().is_ok());
                assert_eq!(*attempts.lock().unwrap(), ["openai-fox", "openai-panda"]);
                assert_eq!(
                    provider.oauth_account_label().as_deref(),
                    Some("openai-panda")
                );
                assert_eq!(sibling.oauth_account_label().as_deref(), Some("openai-fox"));
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-otter")
                );
            }
        });
    });
}

#[test]
fn pool_order_can_choose_explicit_api_before_same_vendor_oauth() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            bind_fixture_account(
                &provider,
                &attempts,
                "openai-otter",
                &[("openai-otter", "429 quota exhausted")],
            );
            let mut pool = crate::auth::account_pool::AccountPool::load();
            pool.members.insert(1, "openai-api".into());
            // Explicit priority may opt a metered identity ahead of subscriptions.
            pool.set_priority("openai-api", 0);
            pool.save().unwrap();
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            let route = prompt.target_route.unwrap();
            assert!(
                route.starts_with("openai-api:"),
                "exact metered identity should follow configured order: {route}"
            );
            assert!(MultiProvider::is_failover_route_pool_approved(&route));
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            pool.members.retain(|key| key != "openai-api");
            pool.excluded.push("openai-api".into());
            pool.save().unwrap();
            assert!(!MultiProvider::is_failover_route_pool_approved(&route));
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-fox:gpt-5.4")
            );
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
        });
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

struct ClaudeSessionRuntime {
    model: std::sync::RwLock<String>,
    credential_mode: std::sync::RwLock<CredentialMode>,
    oauth_account: std::sync::RwLock<Option<String>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl ClaudeSessionRuntime {
    fn unbound(requests: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            model: std::sync::RwLock::new(jcode_provider_core::DEFAULT_CLAUDE_MODEL.to_string()),
            credential_mode: std::sync::RwLock::new(CredentialMode::Auto),
            oauth_account: std::sync::RwLock::new(None),
            requests: Arc::clone(requests),
        }
    }

    fn bound(requests: &Arc<Mutex<Vec<String>>>, label: &str, model: &str) -> Self {
        Self {
            model: std::sync::RwLock::new(model.to_string()),
            credential_mode: std::sync::RwLock::new(CredentialMode::OAuth),
            oauth_account: std::sync::RwLock::new(Some(label.to_string())),
            requests: Arc::clone(requests),
        }
    }
}

#[async_trait::async_trait]
impl Provider for ClaudeSessionRuntime {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> Result<EventStream> {
        self.requests.lock().unwrap().push(self.model());
        Err(anyhow!("429 Too Many Requests"))
    }
    fn name(&self) -> &str {
        "Claude"
    }
    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> Result<()> {
        anyhow::ensure!(
            super::models::known_anthropic_model_ids()
                .iter()
                .any(|known| known == model),
            "Model {model} not supported by Anthropic provider"
        );
        *self.model.write().unwrap() = model.to_string();
        Ok(())
    }
    fn credential_mode(&self) -> CredentialMode {
        *self.credential_mode.read().unwrap()
    }
    fn set_credential_mode(&self, mode: CredentialMode) -> Result<()> {
        *self.credential_mode.write().unwrap() = mode;
        Ok(())
    }
    fn oauth_account_label(&self) -> Option<String> {
        self.oauth_account.read().unwrap().clone()
    }
    fn set_oauth_account(&self, label: Option<&str>) -> Result<()> {
        *self.oauth_account.write().unwrap() = label.map(str::to_owned);
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: std::sync::RwLock::new(self.model()),
            credential_mode: std::sync::RwLock::new(self.credential_mode()),
            oauth_account: std::sync::RwLock::new(self.oauth_account_label()),
            requests: Arc::clone(&self.requests),
        })
    }
}

struct ClaudeFailoverFixtureCleanup;

impl Drop for ClaudeFailoverFixtureCleanup {
    fn drop(&mut self) {
        reset_claude_failover_cooldowns();
    }
}

fn seed_claude_failover_accounts(catalog_models: &[&str]) -> ClaudeFailoverFixtureCleanup {
    reset_claude_failover_cooldowns();
    let mut catalog: Vec<String> = catalog_models.iter().copied().map(str::to_owned).collect();
    if !catalog
        .iter()
        .any(|model| model == jcode_provider_core::DEFAULT_CLAUDE_MODEL)
    {
        catalog.push(jcode_provider_core::DEFAULT_CLAUDE_MODEL.to_string());
    }
    for label in ["claude-otter", "claude-fox"] {
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: label.to_string(),
            access: format!("mock-access-{label}"),
            refresh: format!("mock-refresh-{label}"),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: None,
            subscription_type: Some("max".to_string()),
            scopes: vec!["user:inference".to_string()],
        })
        .expect("save mock Claude account");
        super::models::populate_anthropic_models_for_scope(
            &format!("oauth::{label}"),
            catalog.clone(),
        );
    }
    ClaudeFailoverFixtureCleanup
}

fn claude_multi_provider(runtime: Arc<dyn Provider>) -> MultiProvider {
    MultiProvider {
        anthropic: RwLock::new(Some(runtime)),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
        active_openai_compatible_profile: RwLock::new(None),
        session_id: RwLock::new(None),
        active: RwLock::new(ActiveProvider::Claude),
        startup_notices: RwLock::new(Vec::new()),
        initial_provider: None,
        catalog_scope: next_catalog_scope(),
        routes_memo: std::sync::Mutex::new(None),
        post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
}

async fn claude_failover_target(provider: &dyn Provider) -> Option<String> {
    let error = provider
        .complete(&[], &[], "system", None)
        .await
        .err()
        .expect("return pre-switch prompt");
    parse_failover_prompt_message(&error.to_string())
        .expect("failover prompt")
        .target_route
}

fn reset_claude_failover_cooldowns() {
    for label in ["claude-otter", "claude-fox"] {
        super::models::clear_claude_provider_unavailability_for_account_label(Some(label));
    }
}

#[test]
fn same_provider_account_failover_keeps_the_request_model() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            crate::env::set_var("JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER", "true");
            let request_models = [
                "claude-sonnet-4-6",
                "claude-sonnet-5-5",
                "claude-haiku-5-5",
                "claude-opus-4-8",
            ];
            let _cleanup = seed_claude_failover_accounts(&request_models);
            let mut failed_over = Vec::new();
            for model in request_models {
                let requests = Arc::new(Mutex::new(Vec::new()));
                let provider = claude_multi_provider(Arc::new(ClaudeSessionRuntime::bound(
                    &requests,
                    "claude-otter",
                    model,
                )));
                let target = claude_failover_target(&provider).await;
                failed_over.push((requests.lock().unwrap().clone(), target));
                reset_claude_failover_cooldowns();
            }
            assert_eq!(
                failed_over,
                request_models.map(|model| (
                    vec![model.to_string()],
                    Some(format!("claude-oauth@claude-fox:{model}"))
                )),
                "the account that took over must serve the model that was requested"
            );
        });
    });
}

#[test]
fn same_provider_account_failover_prefers_the_request_model_over_the_configured_default() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            crate::env::set_var("JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER", "true");
            let request_models = ["claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-4-8"];
            let configured_defaults = [
                "claude-sonnet-4-6",
                "claude-opus-5-5",
                "claude-oauth@claude-fox:claude-opus-5-5",
            ];
            let catalog: Vec<&str> = request_models
                .iter()
                .copied()
                .chain(["claude-sonnet-4-6", "claude-opus-5-5"])
                .collect();
            let _cleanup = seed_claude_failover_accounts(&catalog);
            for configured in configured_defaults {
                crate::config::Config::set_default_model(Some(configured), None)
                    .expect("save configured default");
                let mut targets = Vec::new();
                for model in request_models {
                    let requests = Arc::new(Mutex::new(Vec::new()));
                    let provider = claude_multi_provider(Arc::new(ClaudeSessionRuntime::bound(
                        &requests,
                        "claude-otter",
                        model,
                    )));
                    targets.push(claude_failover_target(&provider).await);
                    reset_claude_failover_cooldowns();
                }
                assert_eq!(
                    targets,
                    request_models.map(|model| Some(format!("claude-oauth@claude-fox:{model}"))),
                    "configured default {configured} must not replace an eligible request model"
                );
            }
        });
    });
}

#[test]
fn sessions_forked_from_one_daemon_template_fail_over_with_their_own_model() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            crate::env::set_var("JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER", "true");
            let _cleanup = seed_claude_failover_accounts(&[
                "claude-haiku-5-5",
                "claude-sonnet-5-5",
                "claude-opus-4-8",
            ]);
            crate::auth::claude::set_active_account_override(Some("claude-otter".to_string()));
            let requests = Arc::new(Mutex::new(Vec::new()));
            external::register_external_provider(external::ANTHROPIC_RUNTIME, {
                let requests = Arc::clone(&requests);
                move || Arc::new(ClaudeSessionRuntime::unbound(&requests)) as Arc<dyn Provider>
            });
            let template =
                claude_multi_provider(Arc::new(ClaudeSessionRuntime::unbound(&requests)));

            // Both sessions exist before either selects a model, as when two
            // clients connect to one daemon.
            let haiku_session = template.fork();
            let sonnet_session = template.fork();
            haiku_session
                .set_model("claude-oauth@claude-otter:claude-haiku-5-5")
                .expect("select haiku on a saved login");
            template
                .set_model("claude-opus-4-8")
                .expect("move template");
            sonnet_session
                .set_model("claude-sonnet-5-5")
                .expect("select sonnet on the active login");
            let haiku_helper = haiku_session.fork();
            assert_eq!(
                [
                    template.model(),
                    haiku_session.model(),
                    sonnet_session.model(),
                    haiku_helper.model(),
                ],
                [
                    "claude-opus-4-8",
                    "claude-haiku-5-5",
                    "claude-sonnet-5-5",
                    "claude-haiku-5-5",
                ]
            );

            let sonnet_target = claude_failover_target(sonnet_session.as_ref()).await;
            reset_claude_failover_cooldowns();
            let haiku_target = claude_failover_target(haiku_session.as_ref()).await;
            // The 429 above put the shared login on cooldown, so these two are
            // routed away before they send anything.
            let helper_target = claude_failover_target(haiku_helper.as_ref()).await;
            sonnet_session
                .set_model("claude-oauth@claude-otter:claude-opus-4-8")
                .expect("switch the sonnet session to opus");
            let switched_target = claude_failover_target(sonnet_session.as_ref()).await;
            assert_eq!(
                *requests.lock().unwrap(),
                ["claude-sonnet-5-5", "claude-haiku-5-5"]
            );
            reset_claude_failover_cooldowns();
            let switched_resend = claude_failover_target(sonnet_session.as_ref()).await;
            reset_claude_failover_cooldowns();
            let haiku_resend = claude_failover_target(haiku_session.as_ref()).await;
            assert_eq!(
                *requests.lock().unwrap(),
                [
                    "claude-sonnet-5-5",
                    "claude-haiku-5-5",
                    "claude-opus-4-8",
                    "claude-haiku-5-5",
                ]
            );
            assert_eq!(
                [
                    sonnet_target,
                    haiku_target,
                    helper_target,
                    switched_target,
                    switched_resend,
                    haiku_resend,
                ],
                [
                    "claude-oauth@claude-fox:claude-sonnet-5-5",
                    "claude-oauth@claude-fox:claude-haiku-5-5",
                    "claude-oauth@claude-fox:claude-haiku-5-5",
                    "claude-oauth@claude-fox:claude-opus-4-8",
                    "claude-oauth@claude-fox:claude-opus-4-8",
                    "claude-oauth@claude-fox:claude-haiku-5-5",
                ]
                .map(|route| Some(route.to_string()))
            );
        });
    });
}
