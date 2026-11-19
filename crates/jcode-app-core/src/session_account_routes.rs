use super::*;

pub(crate) fn ensure_bound_account_route(provider: &dyn Provider, session: &Session) -> Result<()> {
    let route = session
        .route_api_method
        .as_deref()
        .or(session.provider_key.as_deref());
    let Some(expected) = route.and_then(jcode_provider_core::oauth_account_label) else {
        return Ok(());
    };
    let actual = provider.oauth_account_label();
    if actual.as_deref() == Some(expected)
        || resolved_bound_account_label(session)
            .is_some_and(|label| actual.as_deref() == Some(label.as_str()))
        || serves_account_via_cli(provider, expected)
    {
        return Ok(());
    }
    anyhow::bail!(
        "Saved account '{}' could not be restored for this session. Select an available account using /model or /login before sending a request.",
        expected
    );
}

/// A saved Claude account runs through its Claude Code CLI instance by
/// default, so a `claude-oauth@<label>` route is honoured by that instance.
fn serves_account_via_cli(provider: &dyn Provider, label: &str) -> bool {
    provider.name() == "Claude Code"
        && crate::auth::claude_code::instance_for_account(label).is_some_and(|instance| {
            provider.resolved_account_label(AccountProviderKind::ClaudeCode) == Some(instance)
        })
}

/// Resolve a persisted saved-account route through the session's identity pin.
/// A label-only route without matching pin evidence must not choose another login.
pub(crate) fn resolved_bound_account_label(session: &Session) -> Option<String> {
    let route = session
        .route_api_method
        .as_deref()
        .or(session.provider_key.as_deref())?;
    let expected = jcode_provider_core::oauth_account_label(route)?;
    let (method, _) = route.split_once('@')?;
    let kind = match method {
        "claude-oauth" => AccountProviderKind::Claude,
        "openai-oauth" => AccountProviderKind::OpenAi,
        _ => return None,
    };
    let pin = session.account_pins.get(kind.key())?;
    let resolved = resolve_pin(kind, pin)?;
    (pin.label == expected || resolved == expected).then_some(resolved)
}

/// Advance a pin's presentation label only when its identity resolves to it.
pub(crate) fn sync_bound_account_pin_label(
    session: &mut Session,
    method: &str,
    label: &str,
) -> bool {
    let kind = match method {
        "claude-oauth" => AccountProviderKind::Claude,
        "openai-oauth" => AccountProviderKind::OpenAi,
        _ => return false,
    };
    let Some(pin) = session.account_pins.get_mut(kind.key()) else {
        return false;
    };
    if pin.label == label || resolve_pin(kind, pin).as_deref() != Some(label) {
        return false;
    }
    pin.label = label.to_owned();
    true
}

/// Canonicalize a saved route before selecting its model on session restore.
/// Only presentation labels change, never credentials or the pinned identity.
pub(crate) fn refresh_bound_account_route(session: &mut Session) {
    let Some(label) = resolved_bound_account_label(session) else {
        return;
    };
    let Some(old) = session
        .route_api_method
        .as_deref()
        .or(session.provider_key.as_deref())
        .map(str::to_string)
    else {
        return;
    };
    let Some((method, expected)) = old.split_once('@') else {
        return;
    };
    sync_bound_account_pin_label(session, method, &label);
    if label == expected {
        return;
    }
    let route = format!("{method}@{label}");
    if session.route_api_method.as_deref() == Some(old.as_str()) {
        session.route_api_method = Some(route.clone());
    }
    if session.provider_key.as_deref() == Some(old.as_str()) {
        session.provider_key = Some(route.clone());
    }
    if let Some(model) = session.model.as_deref()
        && let Some(suffix) = model.strip_prefix(&format!("{old}:"))
    {
        session.model = Some(format!("{route}:{suffix}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LabelProvider(&'static str);

    #[async_trait::async_trait]
    impl Provider for LabelProvider {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            anyhow::bail!("route guard tests must not issue provider requests")
        }

        fn name(&self) -> &str {
            "OpenAI"
        }

        fn oauth_account_label(&self) -> Option<String> {
            Some(self.0.into())
        }

        fn fork(&self) -> std::sync::Arc<dyn Provider> {
            std::sync::Arc::new(Self(self.0))
        }
    }

    struct RestoreHome(Option<std::ffi::OsString>);

    impl Drop for RestoreHome {
        fn drop(&mut self) {
            crate::auth::codex::set_active_account_override(None);
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    #[test]
    fn saved_routes_follow_rename_without_retargeting_another_login() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::auth::codex::set_active_account_override(None);
        let actual = crate::auth::codex::upsert_account_from_tokens(
            "work",
            "access",
            "refresh",
            None,
            Some(i64::MAX),
        )
        .unwrap();
        if actual != "work" {
            crate::auth::codex::rename_account(&actual, "work").unwrap();
        }
        let pin = crate::auth::codex::pin_for_label("work").unwrap();
        let mut session = Session::create(None, None);
        session.route_api_method = Some("openai-oauth@work".into());
        session.provider_key = Some("openai-oauth@work".into());
        session.model = Some("openai-oauth@work:gpt-5.5".into());
        session.account_pins.insert("openai".into(), pin.clone());
        crate::auth::codex::rename_account("work", "Research team").unwrap();
        crate::auth::codex::rename_account("Research team", "Final name").unwrap();
        assert!(ensure_bound_account_route(&LabelProvider("Final name"), &session).is_ok());
        assert!(ensure_bound_account_route(&LabelProvider("unrelated"), &session).is_err());
        let mut legacy = session.clone();
        legacy.account_pins.clear();
        assert!(ensure_bound_account_route(&LabelProvider("Final name"), &legacy).is_err());
        assert_eq!(
            resolved_bound_account_label(&session).as_deref(),
            Some("Final name")
        );
        refresh_bound_account_route(&mut session);
        assert_eq!(
            session.route_api_method.as_deref(),
            Some("openai-oauth@Final name")
        );
        assert_eq!(
            session.provider_key.as_deref(),
            Some("openai-oauth@Final name")
        );
        assert_eq!(
            session.model.as_deref(),
            Some("openai-oauth@Final name:gpt-5.5")
        );
        assert_eq!(session.account_pins["openai"].identity, pin.identity);
        assert_eq!(session.account_pins["openai"].label, "Final name");
        assert!(!sync_bound_account_pin_label(
            &mut session,
            "openai-oauth",
            "unrelated"
        ));
        crate::auth::codex::rename_account("Final name", "Daily work").unwrap();
        assert!(ensure_bound_account_route(&LabelProvider("Daily work"), &session).is_ok());
        refresh_bound_account_route(&mut session);
        assert_eq!(session.account_pins["openai"].label, "Daily work");
        assert_eq!(session.account_pins["openai"].identity, pin.identity);
        session.route_api_method = Some("openai-oauth@unrelated".into());
        assert_eq!(resolved_bound_account_label(&session), None);
        refresh_bound_account_route(&mut session);
        assert_eq!(
            session.route_api_method.as_deref(),
            Some("openai-oauth@unrelated")
        );
        session.route_api_method = Some("openai-api".into());
        assert_eq!(resolved_bound_account_label(&session), None);
    }

    /// Stands in for MultiProvider: `name` is the active runtime, and the
    /// Claude Code instance is reported through the account-pin API.
    struct CliProvider {
        name: &'static str,
        instance: &'static str,
    }

    #[async_trait::async_trait]
    impl Provider for CliProvider {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            anyhow::bail!("route guard tests must not issue provider requests")
        }

        fn name(&self) -> &str {
            self.name
        }

        fn resolved_account_label(&self, kind: AccountProviderKind) -> Option<String> {
            (kind == AccountProviderKind::ClaudeCode).then(|| self.instance.to_string())
        }

        fn fork(&self) -> std::sync::Arc<dyn Provider> {
            std::sync::Arc::new(Self {
                name: self.name,
                instance: self.instance,
            })
        }
    }

    #[test]
    fn claude_account_route_is_served_by_its_cli_instance() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
        let previous_bin = std::env::var_os("JCODE_CLAUDE_CODE_BIN");
        crate::env::set_var("JCODE_HOME", temp.path());
        // Any existing file stands in for the `claude` binary; nothing spawns.
        crate::env::set_var("JCODE_CLAUDE_CODE_BIN", std::env::current_exe().unwrap());
        for label in ["claude-otter", "claude-fox"] {
            crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
                label: label.into(),
                access: "access".into(),
                refresh: format!("refresh-{label}"),
                expires: i64::MAX,
                email: None,
                subscription_type: None,
                scopes: Vec::new(),
            })
            .unwrap();
        }
        let mut session = Session::create(None, None);
        session.route_api_method = Some("claude-oauth@claude-otter".into());
        let cli = |name, instance| CliProvider { name, instance };
        let served = ensure_bound_account_route(&cli("Claude Code", "claude-otter"), &session);
        let other = ensure_bound_account_route(&cli("Claude Code", "claude-fox"), &session);
        let native = ensure_bound_account_route(&cli("Claude", "claude-otter"), &session);
        crate::config::Config::set_claude_account_native("claude-otter", true, None).unwrap();
        let toggled = ensure_bound_account_route(&cli("Claude Code", "claude-otter"), &session);
        match previous_bin {
            Some(value) => crate::env::set_var("JCODE_CLAUDE_CODE_BIN", value),
            None => crate::env::remove_var("JCODE_CLAUDE_CODE_BIN"),
        }
        assert!(served.is_ok(), "{served:?}");
        assert!(other.is_err(), "another account's CLI must not serve it");
        assert!(
            native.is_err(),
            "the native runtime must not pass on a CLI pin"
        );
        assert!(
            toggled.is_err(),
            "a native-mode account needs the native runtime"
        );
    }
}
