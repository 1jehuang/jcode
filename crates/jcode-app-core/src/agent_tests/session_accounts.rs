//! Per-session account pins on the Agent: restore on resume, persistence of
//! a pin that same-provider failover moved (even when the turn then fails),
//! and inheritance by swarm workers.

use super::*;
use crate::provider::{AccountPin, AccountProviderKind};
use std::collections::BTreeMap;
use std::sync::Mutex as StdMutex;

/// Fake provider that records `set_account_pin` calls. When `failover_to` is
/// set, `complete` moves its own pin there (like same-provider failover) and
/// then fails the stream.
#[derive(Clone)]
struct PinProvider {
    pins: Arc<StdMutex<BTreeMap<AccountProviderKind, AccountPin>>>,
    calls: Arc<StdMutex<Vec<(AccountProviderKind, Option<String>)>>>,
    failover: Arc<StdMutex<Option<bool>>>,
    failover_to: Option<AccountPin>,
}

impl PinProvider {
    fn new(failover_to: Option<AccountPin>) -> Self {
        Self {
            pins: Arc::new(StdMutex::new(BTreeMap::new())),
            calls: Arc::new(StdMutex::new(Vec::new())),
            failover: Arc::new(StdMutex::new(None)),
            failover_to,
        }
    }
}

#[async_trait]
impl Provider for PinProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        if let Some(pin) = self.failover_to.clone() {
            self.pins
                .lock()
                .unwrap()
                .insert(AccountProviderKind::Claude, pin);
        }
        let fail = self.failover_to.is_some();
        let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(8);
        tokio::spawn(async move {
            if fail {
                let _ = tx
                    .send(Err(anyhow::anyhow!("connection dropped mid response")))
                    .await;
            } else {
                let _ = tx.send(Ok(StreamEvent::TextDelta("ok".to_string()))).await;
                let _ = tx
                    .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
                    .await;
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }
    fn name(&self) -> &str {
        "pin-provider"
    }
    fn model(&self) -> String {
        "pin-model".to_string()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            pins: Arc::new(StdMutex::new(self.pins.lock().unwrap().clone())),
            calls: Arc::clone(&self.calls),
            failover: Arc::new(StdMutex::new(*self.failover.lock().unwrap())),
            failover_to: self.failover_to.clone(),
        })
    }
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        self.pins.lock().unwrap().get(&kind).cloned()
    }
    fn set_account_pin(&self, kind: AccountProviderKind, pin: Option<AccountPin>) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((kind, pin.as_ref().map(|pin| pin.label.clone())));
        let mut pins = self.pins.lock().unwrap();
        match pin {
            Some(pin) => pins.insert(kind, pin),
            None => pins.remove(&kind),
        };
        Ok(())
    }
    fn set_account_failover(&self, enabled: Option<bool>) {
        *self.failover.lock().unwrap() = enabled;
    }
}

fn store_claude_accounts() {
    for (label, email) in [
        ("claude-otter", "otter@example.com"),
        ("claude-fox", "fox@example.com"),
        ("claude-panda", "panda@example.com"),
    ] {
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: label.to_string(),
            access: format!("access-{label}"),
            refresh: format!("refresh-{label}"),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: Some(email.to_string()),
            subscription_type: Some("max".to_string()),
            scopes: Vec::new(),
        })
        .expect("store account");
    }
}

fn fox_pin() -> AccountPin {
    AccountPin::new("claude-fox", Some("fox@example.com".to_string()))
}

#[tokio::test]
async fn pins_restored_on_resume() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();

    // A session persisted while pinned to fox, with failover turned off.
    let mut saved = Session::create(None, None);
    saved.account_pins.insert("claude".to_string(), fox_pin());
    saved.account_failover = Some(false);
    saved.save_prepared().expect("persist session");

    let provider = PinProvider::new(None);
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    assert_eq!(
        provider
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        None
    );

    agent.restore_session(&saved.id).expect("resume");
    assert_eq!(
        provider
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        Some(&fox_pin()),
        "resuming a pinned session must re-pin its provider"
    );
    assert_eq!(*provider.failover.lock().unwrap(), Some(false));

    // Attaching a live Agent to the persisted session restores it too.
    let provider2 = PinProvider::new(None);
    let provider2_dyn: Arc<dyn Provider> = Arc::new(provider2.clone());
    let registry2 = Registry::new(provider2_dyn.clone()).await;
    let loaded = Session::load(&saved.id).expect("load");
    let _attached = Agent::new_with_session(provider2_dyn, registry2, loaded, None);
    assert_eq!(
        provider2
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        Some(&fox_pin())
    );
}

#[tokio::test]
async fn pin_for_removed_account_is_dropped_on_resume() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();
    let mut saved = Session::create(None, None);
    saved.account_pins.insert(
        "claude".to_string(),
        AccountPin::new("claude-gone", Some("gone@example.com".to_string())),
    );
    saved.save_prepared().expect("persist session");

    let provider = PinProvider::new(None);
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    agent.restore_session(&saved.id).expect("resume");
    assert_eq!(
        provider
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        None
    );
    assert!(agent.account_pins().is_empty());
}

#[tokio::test]
async fn failover_pin_persisted_after_error_exit() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();

    // The session starts unpinned (default otter). During the request the
    // provider fails over to fox, then the stream errors.
    let provider = PinProvider::new(Some(fox_pin()));
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "test".to_string(),
            cache_control: None,
        }],
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let result = agent.run_turn_streaming_mpsc(tx).await;
    assert!(result.is_err(), "the stream error must still end the turn");

    assert_eq!(
        agent.account_pins().get("claude"),
        Some(&fox_pin()),
        "the in-memory session must record the account failover moved to"
    );
    let saved = Session::load(agent.session_id()).expect("session persisted");
    assert_eq!(saved.account_pins.get("claude"), Some(&fox_pin()));

    let mut changed = None;
    while let Ok(event) = rx.try_recv() {
        if let ServerEvent::SessionAccountChanged {
            provider,
            label,
            pinned,
            reason,
            ..
        } = event
        {
            changed = Some((provider, label, pinned, reason));
        }
    }
    let (provider_key, label, pinned, reason) =
        changed.expect("clients must learn the window moved to another account");
    assert_eq!(provider_key, "claude");
    assert_eq!(label.as_deref(), Some("claude-fox"));
    assert!(pinned);
    let reason = reason.expect("a failover move carries a reason");
    assert!(
        reason.contains("claude-otter") && reason.contains("claude-fox"),
        "reason names both accounts: {reason}"
    );
}

/// Provider without per-account support (the trait defaults): `set_account_pin`
/// is a no-op and `account_pin` is always `None`.
struct PinlessProvider;

#[async_trait]
impl Provider for PinlessProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(8);
        tokio::spawn(async move {
            let _ = tx.send(Ok(StreamEvent::TextDelta("ok".to_string()))).await;
            let _ = tx
                .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
                .await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }
    fn name(&self) -> &str {
        "pinless"
    }
    fn model(&self) -> String {
        "pinless-model".to_string()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(PinlessProvider)
    }
}

/// Live regression: a provider that cannot report pins must not look like a
/// failover move, or the post-stream sync erases the user's pin after the
/// first turn.
#[tokio::test]
async fn pin_kept_after_turn_on_provider_without_pin_support() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();
    let provider: Arc<dyn Provider> = Arc::new(PinlessProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent
        .set_account_pin(AccountProviderKind::Claude, Some(fox_pin()))
        .expect("pin");
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "test".to_string(),
            cache_control: None,
        }],
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent.run_turn_streaming_mpsc(tx).await.expect("turn");
    assert_eq!(agent.account_pins().get("claude"), Some(&fox_pin()));
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, ServerEvent::SessionAccountChanged { .. }),
            "no account move happened: {event:?}"
        );
    }
}

#[tokio::test]
async fn swarm_child_inherits_pins() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();

    let provider = PinProvider::new(None);
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut parent = Agent::new(provider_dyn.clone(), registry);
    parent
        .set_account_pin(AccountProviderKind::Claude, Some(fox_pin()))
        .expect("pin parent");
    parent
        .set_account_failover(Some(false))
        .expect("failover off");

    // Headless swarm worker: a fresh provider fork for a new session (no pin)
    // that adopts the coordinator's accounts.
    let child_provider = PinProvider::new(None);
    let child_dyn: Arc<dyn Provider> = Arc::new(child_provider.clone());
    let child_registry = Registry::new(child_dyn.clone()).await;
    let mut child = Agent::new(child_dyn, child_registry);
    child.apply_account_inheritance(&parent.account_inheritance());
    assert_eq!(child.account_pins().get("claude"), Some(&fox_pin()));
    assert_eq!(child.account_failover(), Some(false));
    assert_eq!(
        child_provider
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        Some(&fox_pin()),
        "the worker's provider must bill the coordinator's account"
    );
    assert_eq!(*child_provider.failover.lock().unwrap(), Some(false));

    // Session-level children (visible spawn, split, transfer, overnight) copy
    // the same fields onto the persisted child session.
    let mut child_session = Session::create(None, None);
    parent
        .account_inheritance()
        .apply_to_session(&mut child_session);
    assert_eq!(child_session.account_pins.get("claude"), Some(&fox_pin()));
    assert_eq!(child_session.account_failover, Some(false));
}

/// Rewrite the stored Claude accounts as (label, email) pairs.
fn restore_claude_accounts_as(accounts: &[(&str, &str)]) {
    let mut auth = crate::auth::claude::load_auth_file().expect("auth file");
    auth.anthropic_accounts = accounts
        .iter()
        .map(|(label, email)| crate::auth::claude::AnthropicAccount {
            label: label.to_string(),
            access: format!("access-{email}"),
            refresh: format!("refresh-{email}"),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: Some(email.to_string()),
            subscription_type: Some("max".to_string()),
            scopes: Vec::new(),
        })
        .collect();
    auth.active_anthropic_account = Some(accounts[0].0.to_string());
    crate::auth::claude::save_auth_file(&auth).expect("save auth");
}

/// Greptile "Reused label defeats saved pin": the saved pin's label now
/// belongs to another login. Restore must resolve by identity, re-pin to the
/// identity's new label, or drop the pin and tell the user. It must never
/// keep a pin whose label names a different identity.
#[tokio::test]
async fn restore_rejects_reused_label_and_follows_identity() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();

    // Fox was removed and its label reused by a different subscription.
    restore_claude_accounts_as(&[
        ("claude-otter", "otter@example.com"),
        ("claude-fox", "wolf@example.com"),
    ]);
    let mut saved = Session::create(None, None);
    saved.account_pins.insert("claude".to_string(), fox_pin());
    saved.save_prepared().expect("persist session");

    let provider = PinProvider::new(None);
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    agent.restore_session(&saved.id).expect("resume");
    assert_eq!(
        provider
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        None,
        "a pin whose label now names another login must be dropped"
    );
    assert!(agent.account_pins().is_empty());
    let notices = agent.take_account_notices();
    let reason = notices
        .iter()
        .find_map(|event| match event {
            ServerEvent::SessionAccountChanged {
                provider, reason, ..
            } if provider == "claude" => reason.clone(),
            _ => None,
        })
        .expect("the user is told the pin was dropped");
    assert!(
        reason.contains("claude-fox") && reason.contains("default claude-otter"),
        "{reason}"
    );
    assert!(agent.take_account_notices().is_empty(), "announced once");

    // Fox's login now lives at another label: the window follows it there.
    restore_claude_accounts_as(&[
        ("claude-otter", "otter@example.com"),
        ("claude-fox", "wolf@example.com"),
        ("claude-panda", "fox@example.com"),
    ]);
    let mut saved = Session::create(None, None);
    saved.account_pins.insert("claude".to_string(), fox_pin());
    saved.save_prepared().expect("persist session");
    let provider = PinProvider::new(None);
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    agent.restore_session(&saved.id).expect("resume");
    let expected = AccountPin::new("claude-panda", Some("fox@example.com".to_string()));
    assert_eq!(
        provider
            .pins
            .lock()
            .unwrap()
            .get(&AccountProviderKind::Claude),
        Some(&expected)
    );
    assert_eq!(agent.account_pins().get("claude"), Some(&expected));
}

/// A weak (refresh-hash) identity rotates with every refresh, so a label
/// match still counts.
#[tokio::test]
async fn weak_identity_pin_still_accepts_label_match() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        label: "claude-otter".to_string(),
        access: "access".to_string(),
        refresh: "refresh-now".to_string(),
        expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
        email: None,
        subscription_type: None,
        scopes: Vec::new(),
    })
    .expect("store");
    let pin = AccountPin::new("claude-otter", Some("rt:0123456789abcdef".to_string()));
    assert_eq!(
        crate::session_accounts::resolve_pin(AccountProviderKind::Claude, &pin).as_deref(),
        Some("claude-otter")
    );
}

/// Greptile "Restore notice contradicts replacement": a dropped pin's notice
/// goes out after Subscribe may have pinned a replacement. It must name the
/// account the window really uses.
#[tokio::test]
async fn dropped_pin_notice_names_the_replacement_pin() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    store_claude_accounts();
    restore_claude_accounts_as(&[
        ("claude-otter", "otter@example.com"),
        ("claude-fox", "wolf@example.com"),
    ]);
    let mut saved = Session::create(None, None);
    saved.account_pins.insert("claude".to_string(), fox_pin());
    saved.save_prepared().expect("persist session");

    let provider = PinProvider::new(None);
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    agent.restore_session(&saved.id).expect("resume");
    // Subscribe's `--account` replacement lands before the notice is sent.
    agent
        .set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-fox", Some("wolf@example.com".to_string()))),
        )
        .expect("replacement pin");
    let reason = agent
        .take_account_notices()
        .into_iter()
        .find_map(|event| match event {
            ServerEvent::SessionAccountChanged { reason, .. } => reason,
            _ => None,
        })
        .expect("notice");
    assert!(
        !reason.contains("uses the default") && reason.contains("this window uses claude-fox"),
        "notice contradicts the replacement pin: {reason}"
    );
}
