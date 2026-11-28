use super::*;
use crate::auth::account_pins::{
    self as pins,
    tests::{TestHome, openai_account},
};
use jcode_provider_core::AccountPin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

struct SelectedAccount {
    pin: Mutex<AccountPin>,
    mode: AtomicU8,
}
#[async_trait::async_trait]
impl Provider for SelectedAccount {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        anyhow::bail!("offline identity test never infers");
    }
    fn name(&self) -> &str {
        "OpenAI"
    }
    fn model(&self) -> String {
        "gpt-5.5".into()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            pin: Mutex::new(self.pin.lock().unwrap().clone()),
            mode: AtomicU8::new(self.mode.load(Ordering::Acquire)),
        })
    }
    fn oauth_account_pin(&self) -> Option<AccountPin> {
        Some(self.pin.lock().unwrap().clone())
    }
    fn oauth_account_label(&self) -> Option<String> {
        let pin = self.pin.lock().unwrap();
        Some(pins::resolve_openai(&pin).unwrap_or_else(|_| pin.label.clone()))
    }
    fn set_oauth_account_pin(&self, pin: &AccountPin) -> Result<()> {
        *self.pin.lock().unwrap() = pins::normalize_openai_pin(pin)?;
        Ok(())
    }
    fn credential_mode(&self) -> jcode_provider_core::CredentialMode {
        match self.mode.load(Ordering::Acquire) {
            0 => jcode_provider_core::CredentialMode::Auto,
            1 => jcode_provider_core::CredentialMode::OAuth,
            _ => jcode_provider_core::CredentialMode::ApiKey,
        }
    }
    fn set_credential_mode(&self, mode: jcode_provider_core::CredentialMode) -> Result<()> {
        self.mode.store(
            match mode {
                jcode_provider_core::CredentialMode::Auto => 0,
                jcode_provider_core::CredentialMode::OAuth => 1,
                jcode_provider_core::CredentialMode::ApiKey => 2,
            },
            Ordering::Release,
        );
        Ok(())
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(if self.mode.load(Ordering::Acquire) == 2 {
            jcode_provider_core::ResolvedCredential::ApiKey
        } else {
            jcode_provider_core::ResolvedCredential::Oauth
        })
    }
}

fn selected_session(label: &str) -> (SelectedAccount, Session) {
    let provider = SelectedAccount {
        pin: Mutex::new(pins::openai_pin(label).unwrap()),
        mode: AtomicU8::new(1),
    };
    let route = format!("openai-oauth@{label}");
    let mut session = Session::create_with_id("account_route_identity".into(), None, None);
    session.provider_key = Some(route.clone());
    session.model = Some(format!("{route}:gpt-5.5"));
    session.route_api_method = Some(route);
    record_account_pin(&provider, &mut session);
    (provider, session)
}

#[test]
fn restored_route_and_qualified_model_follow_the_same_identity_after_rename() {
    let _home = TestHome::new();
    let label = openai_account("original", "original-workspace");
    let (provider, mut session) = selected_session(&label);
    let id = session.account_pins["openai"].record_id.clone();
    let mut store = crate::auth::codex::load_auth_file().unwrap();
    store.openai_accounts[0].label = "openai-panda".into();
    crate::auth::codex::save_auth_file(&store).unwrap();
    assert!(ensure_account_identity(&provider, &session).unwrap());
    restore_account_pin_route(&mut session).unwrap();
    assert_eq!(
        session.route_api_method.as_deref(),
        Some("openai-oauth@openai-panda")
    );
    assert_eq!(session.provider_key, session.route_api_method);
    assert_eq!(
        session.model.as_deref(),
        Some("openai-oauth@openai-panda:gpt-5.5")
    );
    apply_restored_account_pin(&provider, &mut session).unwrap();
    assert_eq!(session.account_pins["openai"].record_id, id);
    assert!(ensure_account_identity(&provider, &session).unwrap());
}

#[test]
fn replaced_or_mismatched_identity_blocks_until_explicit_selection_updates_the_pin() {
    let _home = TestHome::new();
    let first = openai_account("first", "first-workspace");
    let other = openai_account("other", "other-workspace");
    let (provider, mut session) = selected_session(&first);
    let mut mismatched = session.clone();
    mismatched
        .account_pins
        .insert("openai".into(), pins::openai_pin(&other).unwrap());
    assert!(ensure_account_identity(&provider, &mismatched).is_err());
    crate::auth::codex::remove_account(&first).unwrap();
    let replacement = openai_account(&first, "replacement-workspace");
    let mut store = crate::auth::codex::load_auth_file().unwrap();
    store
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == replacement)
        .unwrap()
        .label = first.clone();
    crate::auth::codex::save_auth_file(&store).unwrap();
    assert!(restore_account_pin_route(&mut session).is_err());
    assert!(ensure_account_identity(&provider, &session).is_err());
    provider
        .set_oauth_account_pin(&pins::openai_pin(&first).unwrap())
        .unwrap();
    record_account_pin(&provider, &mut session);
    assert!(ensure_account_identity(&provider, &session).unwrap());
    assert_eq!(
        session.account_pins["openai"].identity.as_deref(),
        Some("id:replacement-workspace")
    );
}

#[test]
fn explicit_api_route_keeps_inactive_identity_without_requiring_its_removed_account() {
    let _home = TestHome::new();
    let label = openai_account("first", "first-workspace");
    let (provider, mut session) = selected_session(&label);
    let pins = session.account_pins.clone();
    crate::auth::codex::remove_account(&label).unwrap();
    provider.mode.store(2, Ordering::Release);
    session.route_api_method = Some("openai-api".into());
    session.model = Some("gpt-5.5".into());
    restore_account_pin_route(&mut session).unwrap();
    apply_restored_account_pin(&provider, &mut session).unwrap();
    record_account_pin(&provider, &mut session);
    assert!(!ensure_account_identity(&provider, &session).unwrap());
    assert!(provider.oauth_account_pin().is_some());
    assert!(provider.oauth_account_label().is_some());
    assert!(observed_oauth_account_route(&provider).is_none());
    assert_eq!(session.account_pins, pins);
    assert_eq!(
        provider.active_resolved_credential(),
        Some(jcode_provider_core::ResolvedCredential::ApiKey)
    );
}

#[test]
fn same_local_record_enrichment_does_not_break_the_session_identity_guard() {
    let _home = TestHome::new();
    let label = crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
        record_id: None,
        label: "unknown".into(),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        id_token: None,
        account_id: None,
        email: None,
        expires_at: Some(i64::MAX),
    })
    .unwrap();
    let (provider, session) = selected_session(&label);
    let original = session.account_pins["openai"].clone();
    crate::auth::codex::update_account_profile(&label, Some("hydrated@example.test".into()))
        .unwrap();
    provider.set_oauth_account_pin(&original).unwrap();
    assert_ne!(
        provider.oauth_account_pin().unwrap().identity,
        original.identity
    );
    assert!(ensure_account_identity(&provider, &session).unwrap());
}

#[test]
fn shared_fork_retains_strong_identity_and_api_mode_even_after_the_account_is_removed() {
    let _home = TestHome::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    for target in [
        crate::provider::ActiveProvider::OpenAI,
        crate::provider::ActiveProvider::Claude,
    ] {
        let label = openai_account("retained", "retained-workspace");
        let (runtime, mut session) = selected_session(&label);
        let original = runtime.oauth_account_pin();
        runtime.mode.store(2, Ordering::Release);
        let provider = crate::provider::MultiProvider::new_fast();
        let slot = if target == crate::provider::ActiveProvider::OpenAI {
            &provider.openai
        } else {
            &provider.anthropic
        };
        *slot.write().unwrap() = Some(Arc::new(runtime));
        provider.set_active_provider(target);
        crate::auth::codex::remove_account(&label).unwrap();
        let fork = provider.fork();
        assert_eq!(
            fork.credential_mode(),
            jcode_provider_core::CredentialMode::ApiKey
        );
        assert_eq!(fork.oauth_account_pin(), original);
        assert_eq!(
            fork.active_resolved_credential(),
            Some(jcode_provider_core::ResolvedCredential::ApiKey)
        );
        assert!(fork.oauth_account_label().is_none());
        session.route_api_method = Some("openai-api".into());
        assert!(!ensure_account_identity(fork.as_ref(), &session).unwrap());
    }
}

#[test]
fn shared_auto_binding_keeps_the_existing_pin_and_never_adopts_a_reused_label() {
    let _home = TestHome::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    for target in [
        crate::provider::ActiveProvider::OpenAI,
        crate::provider::ActiveProvider::Claude,
    ] {
        let selected = openai_account("selected", "selected-workspace");
        let other = openai_account("other", "other-workspace");
        let (runtime, session) = selected_session(&selected);
        let original = runtime.oauth_account_pin();
        runtime.mode.store(0, Ordering::Release);
        let provider = crate::provider::MultiProvider::new_fast();
        let slot = if target == crate::provider::ActiveProvider::OpenAI {
            &provider.openai
        } else {
            &provider.anthropic
        };
        *slot.write().unwrap() = Some(Arc::new(runtime));
        provider.set_active_provider(target);
        crate::auth::codex::set_active_account(&other).unwrap();
        provider.bind_auto_account_for_request(target).unwrap();
        assert_eq!(
            provider.credential_mode(),
            jcode_provider_core::CredentialMode::OAuth
        );
        assert_eq!(provider.oauth_account_pin(), original);
        crate::auth::codex::remove_account(&selected).unwrap();
        let mut store = crate::auth::codex::load_auth_file().unwrap();
        store
            .openai_accounts
            .iter_mut()
            .find(|account| account.label == other)
            .unwrap()
            .label = selected.clone();
        crate::auth::codex::save_auth_file(&store).unwrap();
        let fork = provider.fork();
        assert_eq!(
            fork.oauth_account_pin(),
            original,
            "fork must not rebuild a pin from a reused label"
        );
        assert!(ensure_account_identity(fork.as_ref(), &session).is_err());
    }
}
