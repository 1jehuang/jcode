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
        let stream = provider.complete(&messages, &[], "", None).await?;
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

#[test]
fn rotation_skips_exhausted_and_wraps() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        let rt = enter_test_runtime();

        // X on otter runs out: otter is recorded exhausted, X moves to fox.
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 3600,
            },
        );
        let x = account_session(&world, "X", ActiveProvider::Claude, false);
        x.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-otter", None)),
        )
        .unwrap();
        assert_eq!(complete_text(&rt, &x).unwrap(), "hi from claude-fox");
        assert_eq!(world.labels_for("X"), ["claude-otter", "claude-fox"]);

        // Y on panda (the last account) runs out. The rotation wraps to the
        // start (otter), skips otter because it is known to be exhausted
        // until its reset, and lands on fox. Otter gets no doomed request.
        world.set("claude-otter", FakeAccountBehavior::Ok);
        world.set(
            "claude-panda",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 7200,
            },
        );
        let y = account_session(&world, "Y", ActiveProvider::Claude, false);
        y.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-panda", None)),
        )
        .unwrap();
        assert_eq!(complete_text(&rt, &y).unwrap(), "hi from claude-fox");
        assert_eq!(world.labels_for("Y"), ["claude-panda", "claude-fox"]);
    });
}

#[test]
fn all_exhausted_error_names_earliest_reset_and_falls_to_cross_provider_prompt() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        let fox_reset = now + 3600 + 12 * 60;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 3 * 3600,
            },
        );
        world.set(
            "claude-fox",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: fox_reset,
            },
        );
        world.set(
            "claude-panda",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 2 * 3600,
            },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, true);

        let rt = enter_test_runtime();
        let err = complete_text(&rt, &a).expect_err("every Claude account is out");
        let prompt = crate::provider::parse_failover_prompt_message(&err.to_string())
            .unwrap_or_else(|| panic!("expected the cross-provider prompt, got: {err:#}"));
        assert_eq!(prompt.from_provider, "claude");
        assert_eq!(prompt.to_provider, "openai");
        assert!(
            prompt
                .reason
                .contains("All 3 Claude accounts are out of usage."),
            "{}",
            prompt.reason
        );
        assert!(
            prompt
                .reason
                .contains(&format!("First reset: claude-fox at {}", clock(fox_reset))),
            "{}",
            prompt.reason
        );
        assert_eq!(
            world.labels_for("A"),
            ["claude-otter", "claude-fox", "claude-panda"],
            "each account is tried once, nothing is sent to OpenAI"
        );
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude),
            None,
            "the original (unpinned) state is restored"
        );
        assert!(
            a.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
    });
}

#[test]
fn returns_home_after_reset_at_turn_start() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 2;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let rt = enter_test_runtime();
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");

        // Before the reset: stay on fox.
        assert!(a.return_account_home_if_reset().is_empty());
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-fox".to_string())
        );

        std::thread::sleep(std::time::Duration::from_millis(2_200));
        world.set("claude-otter", FakeAccountBehavior::Ok);
        assert_eq!(
            a.return_account_home_if_reset(),
            vec![AccountProviderKind::Claude]
        );
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-otter".to_string())
        );
        assert!(
            a.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-otter");

        // Config off: never returns on its own.
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: chrono::Utc::now().timestamp() + 1,
            },
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        write_provider_config("account_failover_return_home = false");
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert!(a.return_account_home_if_reset().is_empty());
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-fox".to_string())
        );
    });
}

#[test]
fn session_failover_off_disables_rotation() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 3600;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at },
        );
        let rt = enter_test_runtime();

        // This session turned failover off: the usage-limit error reaches the
        // consumer unchanged and no other account is tried.
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        a.set_account_failover(Some(false));
        let err = complete_text(&rt, &a).expect_err("failover is off for this session");
        assert!(err.to_string().contains("429"), "{err:#}");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);
        assert_eq!(a.account_pin(AccountProviderKind::Claude), None);

        // Another session with the config default (on) still moves.
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        assert_eq!(complete_text(&rt, &b).unwrap(), "hi from claude-fox");

        // Session on, config off: the session value wins.
        write_provider_config("same_provider_account_failover = false");
        reset_account_failover_globals();
        let c = account_session(&world, "C", ActiveProvider::Claude, false);
        c.set_account_failover(Some(true));
        assert_eq!(complete_text(&rt, &c).unwrap(), "hi from claude-fox");
    });
}

#[test]
fn short_429_retries_same_account() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let rt = enter_test_runtime();

        // An ordinary 429 inside the stream is not account exhaustion: the
        // runtime owns its retries, the error is replayed untouched, and the
        // session stays on its account.
        world.set("claude-otter", FakeAccountBehavior::InStreamShort429);
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let err = complete_text(&rt, &a).expect_err("the short 429 reaches the consumer");
        assert!(err.to_string().contains("429"), "{err:#}");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);
        assert_eq!(a.account_pin(AccountProviderKind::Claude), None);
        assert!(a.drain_startup_notices().is_empty());

        // Not recorded as exhausted: the next turn goes to the same account.
        world.set("claude-otter", FakeAccountBehavior::Ok);
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-otter");

        // A synchronous short 429 does not move the session either, and
        // never touches the process-wide override.
        world.set(
            "claude-otter",
            FakeAccountBehavior::SyncError(
                "Anthropic API error (429 Too Many Requests): rate limited",
            ),
        );
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        let _ = complete_text(&rt, &b).expect_err("short 429");
        assert_eq!(world.labels_for("B"), ["claude-otter"]);
        assert_eq!(b.account_pin(AccountProviderKind::Claude), None);
        assert_eq!(crate::auth::claude::get_active_account_override(), None);
    });
}

#[test]
fn unavailability_is_per_label() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        write_provider_config("same_provider_account_failover = false");
        let world = Arc::new(FakeAccountWorld::default());
        world.set(
            "claude-otter",
            FakeAccountBehavior::SyncError("401 unauthorized: token revoked"),
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        a.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-otter", None)),
        )
        .unwrap();
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        b.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-fox", None)),
        )
        .unwrap();
        let rt = enter_test_runtime();

        let _ = complete_text(&rt, &a).expect_err("otter is broken");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);

        // Otter's mark must not block a window on fox.
        assert_eq!(complete_text(&rt, &b).unwrap(), "hi from claude-fox");
        assert_eq!(world.labels_for("B"), ["claude-fox"]);

        // Otter itself stays marked: A's next turn sends nothing to it.
        let _ = complete_text(&rt, &a).expect_err("otter is still marked");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);
        assert!(provider_unavailability_detail_for_label("claude", "claude-otter").is_some());
        assert!(provider_unavailability_detail_for_label("claude", "claude-fox").is_none());
    });
}

/// Greptile "Relogin inherits exhausted status": a new subscription logged in
/// under a label that ran out must not inherit its exhausted mark. A token
/// refresh of the same login keeps it.
#[test]
fn relogin_under_exhausted_label_is_not_exhausted() {
    with_account_failover_env(|| {
        let labels = store_claude_accounts(2);
        let otter = labels[0].clone();
        let now = chrono::Utc::now().timestamp();
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            &otter,
            Some(now + 3600),
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                &otter
            )
            .is_some()
        );

        // Same login, refreshed tokens: still exhausted.
        let mut auth = crate::auth::claude::load_auth_file().unwrap();
        let account = auth
            .anthropic_accounts
            .iter_mut()
            .find(|account| account.label == otter)
            .unwrap();
        account.access = "sk-ant-oat01-REFRESHED".to_string();
        account.refresh = "refresh-rotated".to_string();
        crate::auth::claude::save_auth_file(&auth).unwrap();
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                &otter
            )
            .is_some(),
            "a token refresh is the same subscription"
        );

        // Another subscription logged in under the same label.
        let mut auth = crate::auth::claude::load_auth_file().unwrap();
        let account = auth
            .anthropic_accounts
            .iter_mut()
            .find(|account| account.label == otter)
            .unwrap();
        account.email = Some("new-subscription@example.com".to_string());
        account.access = "sk-ant-oat01-NEWLOGIN".to_string();
        account.refresh = "refresh-newlogin".to_string();
        crate::auth::claude::save_auth_file(&auth).unwrap();
        assert_eq!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                &otter
            ),
            None,
            "a new login under the label must not inherit the old exhaustion"
        );
        assert_eq!(
            crate::provider::account_failover::account_resets_at(
                AccountProviderKind::Claude,
                &otter
            ),
            None
        );
    });
}

// ---------------------------------------------------------------------------
// No loops: failover never cycles through accounts that are all out of usage.
// ---------------------------------------------------------------------------

fn claude_all_limited(world: &FakeAccountWorld, resets: &[i64]) {
    for (label, resets_at) in ["claude-otter", "claude-fox", "claude-panda"]
        .into_iter()
        .zip(resets)
    {
        world.set(
            label,
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: *resets_at,
            },
        );
    }
}

/// Guard 1: one pass per call. With 3 accounts all out, exactly 3 requests,
/// then the earliest-reset error. Cross-provider fallback (a configured
/// OpenAI slot) must not re-enter same-provider failover.
#[test]
fn account_failover_all_limited_turn_tries_each_account_once() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        claude_all_limited(&world, &[now + 3 * 3600, now + 3600, now + 2 * 3600]);
        let a = account_session(&world, "A", ActiveProvider::Claude, true);
        let rt = enter_test_runtime();
        let err = complete_text(&rt, &a).expect_err("all out");
        assert!(
            err.to_string()
                .contains("All 3 Claude accounts are out of usage. First reset: claude-fox"),
            "{err:#}"
        );
        assert_eq!(world.requests().len(), 3, "{:?}", world.requests());
        assert_eq!(
            world.labels_for("A"),
            ["claude-otter", "claude-fox", "claude-panda"]
        );
        // Some pre-marked: fewer requests, never more than the stored count.
        reset_account_failover_globals();
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            "claude-fox",
            Some(now + 3600),
        );
        let b = account_session(&world, "B", ActiveProvider::Claude, true);
        let _ = complete_text(&rt, &b).expect_err("all out");
        assert_eq!(world.labels_for("B"), ["claude-otter", "claude-panda"]);
    });
}

/// Guard 2: once every account is marked out, the next turn sends nothing
/// until the earliest reset and fails with the same earliest-reset message.
#[test]
fn account_failover_next_turn_after_all_limited_sends_no_requests() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        claude_all_limited(&world, &[now + 3 * 3600, now + 3600, now + 2 * 3600]);
        let a = account_session(&world, "A", ActiveProvider::Claude, true);
        let rt = enter_test_runtime();
        let first = complete_text(&rt, &a).expect_err("all out");
        assert_eq!(world.requests().len(), 3);
        for turn in 0..3 {
            let err = complete_text(&rt, &a).expect_err("still all out");
            assert!(
                err.to_string()
                    .contains("All 3 Claude accounts are out of usage. First reset: claude-fox"),
                "turn {turn}: {err:#}"
            );
            assert_eq!(world.requests().len(), 3, "turn {turn} sent a request");
        }
        // Another window on the same accounts sends nothing either.
        let b = account_session(&world, "B", ActiveProvider::Claude, true);
        let _ = complete_text(&rt, &b).expect_err("all out");
        assert!(
            world.labels_for("B").is_empty(),
            "{:?}",
            world.labels_for("B")
        );
        drop(first);
    });
}

/// Guard 3: a usage-limit 429 without reset headers keeps the account out
/// for UNKNOWN_RESET_EXHAUSTION_SECS, so later turns do not hammer it.
#[test]
fn account_failover_unknown_reset_not_retried_within_window() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimitNoReset,
        );
        world.set("claude-fox", FakeAccountBehavior::InStreamUsageLimitNoReset);
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let rt = enter_test_runtime();
        let _ = complete_text(&rt, &a).expect_err("both out");
        assert_eq!(world.requests().len(), 2);
        let until = crate::provider::account_failover::account_exhausted(
            AccountProviderKind::Claude,
            "claude-otter",
        )
        .expect("marked");
        let window = crate::provider::account_failover::UNKNOWN_RESET_EXHAUSTION_SECS;
        let now = chrono::Utc::now().timestamp();
        assert!(
            until >= now + window - 5 && until <= now + window,
            "{until}"
        );
        for _ in 0..3 {
            let _ = complete_text(&rt, &a).expect_err("still out");
        }
        assert_eq!(world.requests().len(), 2, "{:?}", world.requests());
    });
}

/// Guard 5: return home only after the recorded reset passed, at most once
/// per reset. Home still limited after returning: record it, move away once,
/// and do not bounce back each turn.
#[test]
fn account_failover_return_home_still_limited_does_not_bounce() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        let rt = enter_test_runtime();
        let soon = chrono::Utc::now().timestamp() + 2;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at: soon },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        assert!(
            a.return_account_home_if_reset().is_empty(),
            "not before the reset"
        );

        std::thread::sleep(std::time::Duration::from_millis(2_200));
        // The provider still reports the old (now past) reset time.
        assert_eq!(
            a.return_account_home_if_reset(),
            vec![AccountProviderKind::Claude]
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        assert_eq!(
            world.labels_for("A"),
            ["claude-otter", "claude-fox", "claude-otter", "claude-fox"],
            "home tried once after its reset, then one move away"
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                "claude-otter"
            )
            .is_some(),
            "a past reset time must not clear the new mark at once"
        );
        for _ in 0..3 {
            assert!(a.return_account_home_if_reset().is_empty(), "no bounce");
            assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        }
        assert_eq!(
            world
                .labels_for("A")
                .iter()
                .filter(|l| *l == "claude-otter")
                .count(),
            2,
            "otter got no more requests: {:?}",
            world.labels_for("A")
        );

        // A new reset time passes: return home once more.
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            "claude-otter",
            Some(chrono::Utc::now().timestamp() + 1),
        );
        world.set("claude-otter", FakeAccountBehavior::Ok);
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert_eq!(
            a.return_account_home_if_reset(),
            vec![AccountProviderKind::Claude]
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-otter");
    });
}

/// Reported bug: an OpenAI account hit its limit, the usage reset early (a
/// banked reset, or the window rolled over), and `/usage` showed it open
/// again, yet every resume kept being skipped with "out of usage (resets
/// 02:10)". The mark held the reported reset time and nothing re-read usage.
/// A fresh snapshot confirming the account is open must clear the mark and
/// the provider cooldown so the resume is actually sent.
#[test]
fn openai_early_reset_clears_usage_limit_mark_and_resume_is_sent() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        let labels = store_openai_accounts(1);
        assert_eq!(labels, ["openai-otter"]);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();

        // The limit was hit with a reset hours away, exactly as in the logs.
        let far_reset = chrono::Utc::now().timestamp() + 5 * 3600;
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(far_reset),
        );
        record_provider_unavailable_for_label(
            "openai",
            "openai-otter",
            "openai-otter is out of usage",
        );
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                hard_limit_reached: true,
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert!(
            complete_text(&rt, &a).is_err(),
            "while really exhausted the request is still skipped"
        );
        assert!(world.labels_for("A").is_empty());

        // A partial or failed snapshot must not be read as "open again".
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                fetched_at: Some(std::time::Instant::now()),
                last_error: Some("API error (500)".to_string()),
                ..Default::default()
            },
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                "openai-otter"
            )
            .is_some(),
            "an error snapshot proves nothing"
        );

        // Fresh usage after the early reset: weekly 0%, ordinary use allowed.
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                seven_day: Some(crate::usage::OpenAIUsageWindow {
                    name: "7-day window".to_string(),
                    usage_ratio: 0.0,
                    resets_at: None,
                }),
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert_eq!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                "openai-otter"
            ),
            None,
            "fresh usage that confirms the reset must clear the mark"
        );
        assert_eq!(
            provider_unavailability_detail_for_label("openai", "openai-otter"),
            None,
            "and the provider cooldown"
        );
        assert_eq!(
            complete_text(&rt, &a).expect("the resume must be sent after the reset"),
            "hi from openai-otter"
        );
        assert_eq!(world.labels_for("A"), ["openai-otter"]);
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
    });
}

/// Serve `body` as the OpenAI usage endpoint for every request, counting
/// requests. Returns the base URL.
fn serve_openai_usage(
    rt: &tokio::runtime::Runtime,
    body: &'static str,
    hits: Arc<std::sync::atomic::AtomicUsize>,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind usage mock");
    let url = format!("http://{}/wham/usage", listener.local_addr().unwrap());
    rt.spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    url
}

const OPENAI_USAGE_OPEN_BODY: &str = r#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":3,"limit_window_seconds":18000},"secondary_window":{"used_percent":10,"limit_window_seconds":604800}}}"#;

/// The live recheck path end to end: the session's own marked account is
/// rechecked against the (mock) usage endpoint, and the resume is sent.
#[test]
fn openai_marked_account_is_rechecked_live_and_the_resume_is_sent() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        store_openai_accounts(1);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        crate::usage::set_openai_usage_url_for_tests(Some(serve_openai_usage(
            &rt,
            OPENAI_USAGE_OPEN_BODY,
            Arc::clone(&hits),
        )));

        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(chrono::Utc::now().timestamp() + 5 * 3600),
        );
        record_provider_unavailable_for_label("openai", "openai-otter", "openai-otter is out of usage");

        let text = complete_text(&rt, &a);
        crate::usage::set_openai_usage_url_for_tests(None);
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
        assert_eq!(text.expect("resume is sent"), "hi from openai-otter");
        assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 1, "usage was fetched live");
    });
}

/// The current account is still out, but a marked alternative reset early.
/// Rotation must recheck the alternative live instead of filtering it out.
#[test]
fn openai_rotation_rechecks_a_marked_alternative_live() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        let labels = store_openai_accounts(2);
        assert_eq!(labels, ["openai-otter", "openai-fox"]);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        crate::usage::set_openai_usage_url_for_tests(Some(serve_openai_usage(
            &rt,
            OPENAI_USAGE_OPEN_BODY,
            Arc::clone(&hits),
        )));

        let far = chrono::Utc::now().timestamp() + 5 * 3600;
        // otter is really out: its recheck is throttled, so it stays marked.
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(far),
        );
        crate::usage::mark_openai_exhaustion_revalidated_for_tests("openai-otter");
        // fox was marked too, but has reset early.
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-fox",
            Some(far),
        );

        let text = complete_text(&rt, &a);
        crate::usage::set_openai_usage_url_for_tests(None);
        crate::usage::forget_openai_usage_for_label_for_tests("openai-fox");
        assert_eq!(
            text.expect("the reset alternative takes the request"),
            "hi from openai-fox"
        );
        assert_eq!(world.labels_for("A"), ["openai-fox"]);
    });
}

/// A usage fetch that started before a new limit was recorded must not clear
/// that newer mark when it lands.
#[test]
fn stale_available_snapshot_does_not_clear_a_newer_exhaustion_mark() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        let started_at = crate::usage::openai_usage_generation_for_tests();
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(chrono::Utc::now().timestamp() + 3600),
        );
        crate::usage::store_openai_usage_for_label_at_generation_for_tests(
            started_at,
            "openai-otter",
            crate::usage::OpenAIUsageData {
                seven_day: Some(crate::usage::OpenAIUsageWindow {
                    name: "7-day window".to_string(),
                    usage_ratio: 0.0,
                    resets_at: None,
                }),
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                "openai-otter"
            )
            .is_some(),
            "the stale snapshot must not clear the newer mark"
        );
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
    });
}

/// An available snapshot says nothing about credentials. A 401/403 cooldown
/// must stay until it expires.
#[test]
fn available_snapshot_keeps_an_auth_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_label(
            "openai",
            "openai-otter",
            "OpenAI API error (401 Unauthorized): token expired",
        );
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                seven_day: Some(crate::usage::OpenAIUsageWindow {
                    name: "7-day window".to_string(),
                    usage_ratio: 0.0,
                    resets_at: None,
                }),
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert!(
            provider_unavailability_detail_for_label("openai", "openai-otter").is_some(),
            "an auth cooldown is not a usage limit"
        );
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
    });
}
