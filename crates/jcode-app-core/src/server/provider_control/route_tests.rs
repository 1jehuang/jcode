#![allow(clippy::await_holding_lock)]
use super::*;
use crate::auth::claude_cli_completion::{self as completion, LoginFlow, LoginSource};
use std::sync::RwLock;

struct Home {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    previous_override: Option<String>,
    _sandbox: crate::auth::test_sandbox::AuthTestSandbox,
}
impl Home {
    fn new() -> Self {
        let previous_override = crate::auth::claude::get_active_account_override();
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let mut saved = Vec::new();
        for key in [
            "HOME",
            "JCODE_GLOBAL_HOME",
            "JCODE_RUNTIME_DIR",
            "CLAUDE_CONFIG_DIR",
        ] {
            saved.push((key, std::env::var_os(key)));
            crate::env::set_var(key, sandbox.root().join(key));
        }
        for key in [
            "JCODE_TEST_SESSION",
            "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            saved.push((key, std::env::var_os(key)));
            crate::env::remove_var(key);
        }
        crate::env::set_var("JCODE_TEST_SESSION", "1");
        crate::auth::claude::set_active_account_override(None);
        Self {
            saved,
            previous_override,
            _sandbox: sandbox,
        }
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        crate::auth::claude::set_active_account_override(self.previous_override.take());
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        crate::config::Config::invalidate_cache();
    }
}

struct VerifiedProvider {
    model: RwLock<String>,
    account: RwLock<Option<String>>,
    fingerprint: String,
    bound: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl Provider for VerifiedProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        panic!("offline route test must not infer");
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: RwLock::new(self.model()),
            account: RwLock::new(self.oauth_account_label()),
            fingerprint: self.fingerprint.clone(),
            bound: std::sync::atomic::AtomicBool::new(self.bound.load(Ordering::Acquire)),
        })
    }
    fn name(&self) -> &str {
        "Anthropic"
    }
    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }
    fn oauth_account_label(&self) -> Option<String> {
        self.bound
            .load(Ordering::Acquire)
            .then(|| self.account.read().unwrap().clone())
            .flatten()
    }
    fn serving_oauth_account_label(&self) -> Option<String> {
        self.account.read().unwrap().clone()
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(jcode_provider_core::ResolvedCredential::Oauth)
    }
    fn set_route_selection_verified(
        &self,
        route: &RouteSelection,
        fingerprint: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(fingerprint == self.fingerprint, "wrong daemon grant");
        *self.model.write().unwrap() = route.model.clone();
        *self.account.write().unwrap() = route.account_label().map(str::to_owned);
        self.bound.store(true, Ordering::Release);
        Ok(())
    }
}

fn save_account(label: &str) -> String {
    crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        record_id: None,
        label: label.into(),
        access: label.into(),
        refresh: format!("{label}-refresh"),
        expires: i64::MAX,
        email: None,
        subscription_type: None,
        scopes: vec![],
    })
    .unwrap()
}

#[tokio::test]
async fn deferred_sign_in_route_checks_attempt_origin_source_and_exact_grant_at_apply() {
    for condition in [
        "success", "unbound", "cancel", "revoke", "model", "account", "grant",
    ] {
        let success = matches!(condition, "success" | "unbound");
        let _home = Home::new();
        let previous = save_account("previous");
        let selected = save_account("selected");
        crate::auth::claude::trust_native_source().unwrap();
        crate::auth::claude::set_active_account(&selected).unwrap();
        let credentials = crate::auth::claude::load_credentials_for_account(&selected).unwrap();
        let completed = completion::capture(&credentials, LoginSource::Native).unwrap();
        let flow = LoginFlow::new().unwrap();
        completion::claim(&flow.path).unwrap();
        completion::finish(&flow.path, &flow.attempt, Some(completed.clone())).unwrap();
        let provider = Arc::new(VerifiedProvider {
            model: RwLock::new("claude-sonnet-4-6".into()),
            account: RwLock::new(Some(previous.clone())),
            bound: std::sync::atomic::AtomicBool::new(condition != "unbound"),
            fingerprint: if condition == "grant" {
                "different-grant".into()
            } else {
                completed.fingerprint.clone()
            },
        });
        let provider_dyn: Arc<dyn Provider> = provider.clone();
        let registry = crate::tool::Registry::new(Arc::clone(&provider_dyn)).await;
        let session =
            crate::session::Session::create_with_id(format!("claude_cli_{condition}"), None, None);
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            provider_dyn,
            registry,
            session,
            None,
        )));
        let verification = jcode_provider_core::AuthRouteVerification {
            fingerprint: completed.fingerprint,
            completion_path: flow.path.clone(),
            attempt: flow.attempt.clone(),
            previous_model: provider.model(),
            previous_provider: provider.name().into(),
            previous_serving_account_label: Some(previous.clone()),
            previous_credential: provider.active_resolved_credential(),
        };
        let selection = RouteSelection {
            model: provider.model(),
            runtime_key: jcode_provider_core::RuntimeKey::ClaudeOAuth,
            api_method: format!("claude-oauth@{selected}"),
            provider_label: "Claude".into(),
            detail: String::new(),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let held = agent.lock().await;
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            handle_set_route_verified(91, selection, false, Some(verification), &agent, &tx),
        )
        .await
        .unwrap();
        assert!(rx.try_recv().is_err());
        match condition {
            "cancel" => flow.cancel().unwrap(),
            "revoke" => crate::config::Config::revoke_external_auth_source(
                crate::auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
            )
            .unwrap(),
            "model" => *provider.model.write().unwrap() = "manual-choice".into(),
            "account" => *provider.account.write().unwrap() = Some("manual-account".into()),
            _ => {}
        }
        drop(held);
        let proof = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(proof, ServerEvent::AuthRouteVerified { id: 91, verified } if verified == success),
            "{condition}: {proof:?}"
        );
        let ack = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(ack, ServerEvent::ModelChanged { id: 91, error, .. } if error.is_none() == success)
        );
        assert_eq!(
            provider.oauth_account_label().as_deref(),
            Some(if success {
                selected.as_str()
            } else if condition == "account" {
                "manual-account"
            } else {
                previous.as_str()
            })
        );
        assert_eq!(
            provider.model(),
            if condition == "model" {
                "manual-choice"
            } else {
                "claude-sonnet-4-6"
            }
        );
    }
}
