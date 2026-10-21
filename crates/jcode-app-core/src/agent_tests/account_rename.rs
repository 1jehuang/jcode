use super::*;
use crate::auth::{AccountPin, AccountProviderKind};

struct RenameAwareProvider {
    kind: AccountProviderKind,
    pin: AccountPin,
}

#[async_trait]
impl Provider for RenameAwareProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("rename regression must not send a network request")
    }
    fn name(&self) -> &str {
        match self.kind {
            AccountProviderKind::Claude => "Claude",
            _ => "OpenAI",
        }
    }
    fn model(&self) -> String {
        "rename-test-model".into()
    }
    fn oauth_account_label(&self) -> Option<String> {
        match self.kind {
            AccountProviderKind::Claude => crate::auth::claude::resolve_pin(&self.pin),
            _ => crate::auth::codex::resolve_pin(&self.pin),
        }
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(jcode_provider_core::ResolvedCredential::Oauth)
    }
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == self.kind).then(|| self.pin.clone())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            kind: self.kind,
            pin: self.pin.clone(),
        })
    }
}

#[tokio::test]
async fn interleaved_account_renames_preserve_live_agent_lease() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::auth::claude::set_active_account_override(None);
            crate::auth::codex::set_active_account_override(None);
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    for (kind, method) in [
        (AccountProviderKind::Claude, "claude-oauth"),
        (AccountProviderKind::OpenAi, "openai-oauth"),
    ] {
        let actual = match kind {
            AccountProviderKind::Claude => {
                crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
                    label: "work".into(),
                    access: "synthetic-access".into(),
                    refresh: "synthetic-refresh".into(),
                    expires: i64::MAX,
                    email: Some("original@example.invalid".into()),
                    subscription_type: None,
                    scopes: vec![],
                })
                .unwrap()
            }
            _ => {
                let label = crate::auth::codex::upsert_account_from_tokens(
                    "work",
                    "synthetic-access",
                    "synthetic-refresh",
                    None,
                    Some(i64::MAX),
                )
                .unwrap();
                crate::auth::codex::update_account_profile(
                    &label,
                    Some("original@example.invalid".into()),
                )
                .unwrap();
                label
            }
        };
        let rename = |old: &str, new: &str| match kind {
            AccountProviderKind::Claude => crate::auth::claude::rename_account(old, new),
            _ => crate::auth::codex::rename_account(old, new),
        };
        rename(&actual, "work").unwrap();
        let pin = match kind {
            AccountProviderKind::Claude => crate::auth::claude::pin_for_label("work"),
            _ => crate::auth::codex::pin_for_label("work"),
        }
        .unwrap();
        let mut agent = Agent::new(
            Arc::new(RenameAwareProvider {
                kind,
                pin: pin.clone(),
            }),
            Registry::empty(),
        );
        agent
            .session
            .account_pins
            .insert(kind.key().into(), pin.clone());
        agent.session.route_api_method = Some(format!("{method}@work"));
        let mut previous = "work";
        for label in ["Research team", "Final name", "Daily work"] {
            rename(previous, label).unwrap();
            assert!(
                agent.ensure_session_lease().is_ok(),
                "{method}: {previous} -> {label}"
            );
            agent.sync_bound_account_route();
            assert_eq!(
                agent.session.route_api_method.as_deref(),
                Some(format!("{method}@{label}").as_str())
            );
            assert_eq!(
                agent.session.account_pins[kind.key()].identity,
                pin.identity
            );
            assert_eq!(agent.session.account_pins[kind.key()].label, label);
            previous = label;
        }
        let replacement = match kind {
            AccountProviderKind::Claude => {
                crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
                    label: "work".into(),
                    access: "replacement-access".into(),
                    refresh: "replacement-refresh".into(),
                    expires: i64::MAX,
                    email: Some("replacement@example.invalid".into()),
                    subscription_type: None,
                    scopes: vec![],
                })
                .unwrap()
            }
            _ => {
                let label = crate::auth::codex::upsert_account_from_tokens(
                    "work",
                    "replacement-access",
                    "replacement-refresh",
                    None,
                    Some(i64::MAX),
                )
                .unwrap();
                crate::auth::codex::update_account_profile(
                    &label,
                    Some("replacement@example.invalid".into()),
                )
                .unwrap();
                label
            }
        };
        rename(&replacement, "work").unwrap();
        assert!(
            agent.ensure_session_lease().is_ok(),
            "reusing a previous label must not retarget the live account"
        );
        match kind {
            AccountProviderKind::Claude => crate::auth::claude::remove_account(previous).unwrap(),
            _ => crate::auth::codex::remove_account(previous).unwrap(),
        }
        assert!(
            agent.ensure_session_lease().is_err(),
            "removing the pinned identity must fail closed"
        );
    }
}
