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
    {
        return Ok(());
    }
    anyhow::bail!(
        "Saved account '{}' could not be restored for this session. Select an available account using /model or /login before sending a request.",
        expected
    );
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

/// Canonicalize a saved route before selecting its model on session restore.
/// This changes presentation metadata only, never the credential or account pin.
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
        assert_eq!(session.account_pins["openai"], pin);
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
}
