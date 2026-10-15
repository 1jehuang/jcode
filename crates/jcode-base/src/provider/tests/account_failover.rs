// Per-session same-provider account failover (slice S3).
//
// Fake Claude/OpenAI runtimes implement the account-pin trait methods
// themselves (so these tests do not depend on the real S2 plumbing) and
// resolve the account per request: the pin, else the stored default account.
// A shared `FakeAccountWorld` decides how each account answers and records
// which account served every request.

use jcode_provider_core::{AccountPin, AccountProviderKind};

#[derive(Clone, Debug)]
enum FakeAccountBehavior {
    /// Streams "hi from <label>".
    Ok,
    /// HTTP 200 stream whose first item is a marked usage-limit 429.
    InStreamUsageLimit { resets_at: i64 },
    /// A marked usage-limit 429 with no reset time (no reset headers).
    InStreamUsageLimitNoReset,
    /// Same, delivered as a `StreamEvent::Error` (OpenAI WebSocket shape).
    InStreamUsageLimitEvent { resets_at: i64 },
    /// An ordinary short 429 inside the stream (no usage-limit marker).
    InStreamShort429,
    /// A synchronous error before any stream exists.
    SyncError(&'static str),
}

#[derive(Default)]
struct FakeAccountWorld {
    behavior: std::sync::Mutex<std::collections::HashMap<String, FakeAccountBehavior>>,
    /// (session tag, label) per request, in order.
    requests: std::sync::Mutex<Vec<(&'static str, String)>>,
}

impl FakeAccountWorld {
    fn set(&self, label: &str, behavior: FakeAccountBehavior) {
        self.behavior
            .lock()
            .unwrap()
            .insert(label.to_string(), behavior);
    }

    fn behavior(&self, label: &str) -> FakeAccountBehavior {
        self.behavior
            .lock()
            .unwrap()
            .get(label)
            .cloned()
            .unwrap_or(FakeAccountBehavior::Ok)
    }

    fn requests(&self) -> Vec<(&'static str, String)> {
        self.requests.lock().unwrap().clone()
    }

    fn labels_for(&self, session: &str) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|(tag, _)| *tag == session)
            .map(|(_, label)| label)
            .collect()
    }
}

struct FakeAccountRuntime {
    kind: AccountProviderKind,
    session: &'static str,
    world: Arc<FakeAccountWorld>,
    pin: std::sync::RwLock<Option<AccountPin>>,
    model: std::sync::RwLock<String>,
}

impl FakeAccountRuntime {
    fn new(
        kind: AccountProviderKind,
        session: &'static str,
        world: Arc<FakeAccountWorld>,
    ) -> Arc<Self> {
        let model = match kind {
            AccountProviderKind::Claude => "claude-opus-4-6",
            AccountProviderKind::OpenAi => "gpt-5.4",
        };
        Arc::new(Self {
            kind,
            session,
            world,
            pin: std::sync::RwLock::new(None),
            model: std::sync::RwLock::new(model.to_string()),
        })
    }

    /// Pin, else the stored default account (like the real scoped load).
    fn current_label(&self) -> String {
        if let Some(pin) = self.pin.read().unwrap().as_ref() {
            return pin.label.clone();
        }
        match self.kind {
            AccountProviderKind::Claude => crate::auth::claude::active_account_label(),
            AccountProviderKind::OpenAi => crate::auth::codex::active_account_label(),
        }
        .unwrap_or_else(|| "default".to_string())
    }
}

#[async_trait::async_trait]
impl Provider for FakeAccountRuntime {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        use crate::message::{ConnectionPhase, StreamEvent};
        let label = self.current_label();
        self.world
            .requests
            .lock()
            .unwrap()
            .push((self.session, label.clone()));
        let connecting: anyhow::Result<StreamEvent> = Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        });
        let items: Vec<anyhow::Result<StreamEvent>> = match self.world.behavior(&label) {
            FakeAccountBehavior::SyncError(message) => anyhow::bail!("{message}"),
            FakeAccountBehavior::Ok => vec![
                connecting,
                Ok(StreamEvent::TextDelta(format!("hi from {label}"))),
                Ok(StreamEvent::MessageEnd { stop_reason: None }),
            ],
            FakeAccountBehavior::InStreamUsageLimit { resets_at } => vec![
                connecting,
                Err(anyhow::anyhow!(
                    "Anthropic API error (429 Too Many Requests): rate_limit_error Usage limit reached for this Claude account. {}",
                    jcode_provider_core::account_usage_limit_marker(Some(resets_at))
                )),
            ],
            FakeAccountBehavior::InStreamUsageLimitNoReset => vec![
                connecting,
                Err(anyhow::anyhow!(
                    "Anthropic API error (429 Too Many Requests): rate_limit_error Usage limit reached for this Claude account. {}",
                    jcode_provider_core::account_usage_limit_marker(None)
                )),
            ],
            FakeAccountBehavior::InStreamUsageLimitEvent { resets_at } => vec![
                connecting,
                Ok(StreamEvent::Error {
                    message: format!(
                        "usage_limit_reached: The usage limit has been reached {}",
                        jcode_provider_core::account_usage_limit_marker(Some(resets_at))
                    ),
                    retry_after_secs: None,
                }),
            ],
            FakeAccountBehavior::InStreamShort429 => vec![
                connecting,
                Err(anyhow::anyhow!(
                    "Anthropic API error (429 Too Many Requests): rate_limit_error"
                )),
            ],
        };
        Ok(Box::pin(futures::stream::iter(items)))
    }

    fn name(&self) -> &'static str {
        match self.kind {
            AccountProviderKind::Claude => "anthropic",
            AccountProviderKind::OpenAi => "openai",
        }
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        *self.model.write().unwrap() = model.to_string();
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let fork = Self::new(self.kind, self.session, Arc::clone(&self.world));
        *fork.pin.write().unwrap() = self.pin.read().unwrap().clone();
        fork
    }

    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == self.kind)
            .then(|| self.pin.read().unwrap().clone())
            .flatten()
    }

    fn set_account_pin(
        &self,
        kind: AccountProviderKind,
        pin: Option<AccountPin>,
    ) -> anyhow::Result<()> {
        if kind == self.kind {
            *self.pin.write().unwrap() = pin;
        }
        Ok(())
    }

    fn oauth_account_label(&self) -> Option<String> {
        self.pin
            .read()
            .unwrap()
            .as_ref()
            .map(|pin| pin.label.clone())
    }

    fn set_oauth_account(&self, label: Option<&str>) -> Result<()> {
        self.set_account_pin(self.kind, label.map(|label| AccountPin::new(label, None)))
    }

    fn resolved_account_label(&self, kind: AccountProviderKind) -> Option<String> {
        (kind == self.kind).then(|| self.current_label())
    }
}

/// One session: a MultiProvider whose Claude (and optionally OpenAI) slot is
/// a fake runtime tagged `session`.
fn account_session(
    world: &Arc<FakeAccountWorld>,
    session: &'static str,
    active: ActiveProvider,
    with_openai: bool,
) -> MultiProvider {
    let anthropic =
        FakeAccountRuntime::new(AccountProviderKind::Claude, session, Arc::clone(world));
    let openai = (with_openai || active == ActiveProvider::OpenAI).then(|| {
        FakeAccountRuntime::new(AccountProviderKind::OpenAi, session, Arc::clone(world))
            as Arc<dyn Provider>
    });
    MultiProvider {
        anthropic: RwLock::new(Some(anthropic as Arc<dyn Provider>)),
        openai: RwLock::new(openai),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
        active_openai_compatible_profile: RwLock::new(None),
        active: RwLock::new(active),
        startup_notices: RwLock::new(Vec::new()),
        initial_provider: None,
        catalog_scope: next_catalog_scope(),
        routes_memo: std::sync::Mutex::new(None),
        post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        account_failover: Default::default(),
    }
}

/// Store `count` Claude accounts (claude-otter, claude-fox, claude-panda...).
fn store_claude_accounts(count: usize) -> Vec<String> {
    let expires = chrono::Utc::now().timestamp_millis() + 3_600_000;
    (0..count)
        .map(|index| {
            crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
                label: format!("seed-{index}"),
                access: format!("sk-ant-oat01-ACCT-{index}"),
                refresh: format!("refresh-{index}"),
                expires,
                email: Some(format!("user{index}@example.com")),
                subscription_type: None,
                scopes: Vec::new(),
            })
            .expect("store claude account")
        })
        .collect()
}

fn store_openai_accounts(count: usize) -> Vec<String> {
    let expires = chrono::Utc::now().timestamp_millis() + 3_600_000;
    (0..count)
        .map(|index| {
            crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
                label: format!("seed-{index}"),
                access_token: format!("acc-{index}"),
                refresh_token: format!("ref-{index}"),
                id_token: None,
                account_id: Some(format!("acct-{index}")),
                expires_at: Some(expires),
                email: Some(format!("o{index}@example.com")),
            })
            .expect("store openai account")
        })
        .collect()
}

fn write_provider_config(body: &str) {
    let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
    std::fs::write(
        std::path::PathBuf::from(jcode_home).join("config.toml"),
        format!("[provider]\n{body}\n"),
    )
    .expect("write test config.toml");
    crate::config::invalidate_config_cache();
}

fn reset_account_failover_globals() {
    crate::provider::account_failover::reset_account_exhaustion_for_tests();
    clear_all_provider_unavailability_for_account();
    for label in ["claude-otter", "claude-fox", "claude-panda"] {
        clear_provider_unavailable_for_label("claude", label);
    }
    for label in ["openai-otter", "openai-fox"] {
        clear_provider_unavailable_for_label("openai", label);
    }
}

fn with_account_failover_env<T>(f: impl FnOnce() -> T) -> T {
    with_clean_provider_test_env(|| {
        reset_account_failover_globals();
        let result = f();
        reset_account_failover_globals();
        result
    })
}

fn complete_text(rt: &tokio::runtime::Runtime, provider: &MultiProvider) -> anyhow::Result<String> {
    let messages = billing_test_messages();
    rt.block_on(async {
        // Exercise the explicit low-level session rotation helper here. The
        // production router separately proves countdown approval before any
        // alternate request in the upstream tests below.
        let active = provider.active_provider();
        let stream = if let Some(accounts) = provider.account_failover_labels(active) {
            let known = provider_unavailability_detail_for_label(
                MultiProvider::provider_key(active),
                &accounts.1,
            );
            match provider
                .complete_with_account_failover(
                    active,
                    accounts,
                    known,
                    &messages,
                    &[],
                    CompletionMode::Unified { system: "" },
                    None,
                    &mut Vec::new(),
                )
                .await
            {
                super::multi_provider::AccountAttempt::Stream(stream) => stream,
                super::multi_provider::AccountAttempt::Error(error) => return Err(error),
                super::multi_provider::AccountAttempt::Exhausted(reason) => {
                    anyhow::bail!(reason)
                }
            }
        } else {
            provider.complete(&messages, &[], "", None).await?
        };
        drain_text(stream).await
    })
}

fn clock(unix: i64) -> String {
    crate::provider::account_failover::format_reset_clock(unix)
}

#[test]
fn in_stream_claude_usage_limit_triggers_account_failover() {
    with_account_failover_env(|| {
        let labels = store_claude_accounts(3);
        assert_eq!(labels, ["claude-otter", "claude-fox", "claude-panda"]);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 3 * 3600 + 17 * 60;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);

        let rt = enter_test_runtime();
        let text = complete_text(&rt, &a).expect("the turn must finish on another account");
        assert_eq!(text, "hi from claude-fox");
        assert_eq!(world.labels_for("A"), ["claude-otter", "claude-fox"]);
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-fox".to_string()),
            "only this session is pinned to the next account"
        );
        assert_eq!(
            a.account_failover_home(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-otter".to_string())
        );
        let notices = a.drain_startup_notices();
        assert!(
            notices.iter().any(|notice| notice
                == &format!(
                    "⚡ This window moved from claude-otter to claude-fox (claude-otter resets {})",
                    clock(resets_at)
                )),
            "{notices:?}"
        );
        // The stored default and the process-wide override are untouched.
        assert_eq!(crate::auth::claude::get_active_account_override(), None);
        assert_eq!(
            crate::auth::claude::active_account_label().as_deref(),
            Some("claude-otter")
        );
    });
}

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
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == AccountProviderKind::OpenAi && self.credential_mode() == CredentialMode::OAuth)
            .then(|| AccountPin::new(self.label.lock().unwrap().clone(), None))
    }
    fn set_account_pin(&self, kind: AccountProviderKind, pin: Option<AccountPin>) -> Result<()> {
        if kind == AccountProviderKind::OpenAi
            && let Some(pin) = pin
        {
            *self.label.lock().unwrap() = pin.label;
        }
        Ok(())
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
fn in_stream_codex_usage_limit_reached_triggers_account_failover() {
    with_account_failover_env(|| {
        let labels = store_openai_accounts(2);
        assert_eq!(labels, ["openai-otter", "openai-fox"]);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 2 * 3600;
        world.set(
            "openai-otter",
            FakeAccountBehavior::InStreamUsageLimitEvent { resets_at },
        );
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);

        let rt = enter_test_runtime();
        let text = complete_text(&rt, &a).expect("the turn must finish on openai-fox");
        assert_eq!(text, "hi from openai-fox");
        assert_eq!(world.labels_for("A"), ["openai-otter", "openai-fox"]);
        assert_eq!(
            a.account_pin(AccountProviderKind::OpenAi)
                .map(|pin| pin.label),
            Some("openai-fox".to_string())
        );
        assert_eq!(crate::auth::codex::get_active_account_override(), None);
        assert_eq!(
            crate::auth::codex::active_account_label().as_deref(),
            Some("openai-otter")
        );
    });
}

#[test]
fn failover_moves_only_this_session() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 3600;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        b.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-panda", None)),
        )
        .unwrap();

        let rt = enter_test_runtime();
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");

        // B keeps its own pin, the default is still otter, no global override.
        assert_eq!(
            b.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-panda".to_string())
        );
        assert_eq!(crate::auth::claude::get_active_account_override(), None);
        assert_eq!(
            crate::auth::claude::active_account_label().as_deref(),
            Some("claude-otter")
        );
        assert!(b.drain_startup_notices().is_empty());
        assert!(
            b.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
        assert_eq!(complete_text(&rt, &b).unwrap(), "hi from claude-panda");
        assert_eq!(world.labels_for("B"), ["claude-panda"]);
    });
}

include!("account_failover_session.rs");
