use super::*;

impl MultiProvider {
    /// Auto prefers saved subscriptions, but provider-level Auto can silently
    /// fall back to an API key when refresh fails. Bind the subscription before
    /// sending anything so only the pool/countdown router may change billing.
    pub(super) fn bind_auto_account_for_request(&self, target: ActiveProvider) -> Result<()> {
        if target == ActiveProvider::Claude
            && std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok_and(|token| !token.trim().is_empty())
        {
            // External OAuth tokens retain their existing precedence and identity.
            return Ok(());
        }
        let runtime = match target {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => return Ok(()),
        };
        let Some(runtime) =
            runtime.filter(|runtime| runtime.credential_mode() == CredentialMode::Auto)
        else {
            return Ok(());
        };
        let Some(label) = active_account_label_for_provider(target) else {
            return Ok(());
        };
        let saved = match target {
            ActiveProvider::Claude => crate::auth::claude::list_accounts()?
                .iter()
                .any(|account| account.label == label),
            ActiveProvider::OpenAI => crate::auth::codex::list_accounts()?
                .iter()
                .any(|account| account.label == label),
            _ => false,
        };
        if !saved {
            return Ok(());
        }
        // A freshly spawned session inherits the process-wide account. If that
        // login is known to be spent (local cooldown, no network IO), start on
        // the next ready pool member instead of failing the first request.
        let label = match models::provider_unavailability_detail_for_account_label(
            Self::provider_key(target),
            Some(&label),
        ) {
            Some(reason) if same_provider_account_failover_enabled() => {
                match account_failover::same_provider_account_candidates(target)
                    .into_iter()
                    .next()
                {
                    Some(next) => {
                        crate::logging::info(&format!(
                            "{} account '{}' unavailable ({}), routing new request to '{}'",
                            Self::provider_label(target),
                            label,
                            reason,
                            next
                        ));
                        self.startup_notices
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(format!(
                                "⚡ Routed {} account: **{}** -> **{}** (previous account unavailable)",
                                Self::provider_label(target),
                                label,
                                next
                            ));
                        next
                    }
                    None => label,
                }
            }
            _ => label,
        };
        let replacement = runtime.fork();
        replacement.set_oauth_account(Some(&label))?;
        replacement.set_credential_mode(CredentialMode::OAuth)?;
        let slot = match target {
            ActiveProvider::Claude => &self.anthropic,
            ActiveProvider::OpenAI => &self.openai,
            _ => unreachable!(),
        };
        let mut current = slot.write().unwrap_or_else(|p| p.into_inner());
        if current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &runtime))
        {
            *current = Some(replacement);
            self.invalidate_routes_memo();
        }
        Ok(())
    }

    /// Select a saved login without rewriting the process-wide account preference.
    /// Build the replacement off to the side so validation errors leave the old
    /// runtime, model and account untouched.
    pub(super) fn set_model_on_saved_account(
        &self,
        method: &str,
        label: &str,
        model: &str,
    ) -> Result<()> {
        if label.is_empty() || model.trim().is_empty() {
            anyhow::bail!("Account label and model cannot be empty");
        }
        let target = if method == "claude-oauth" {
            if !crate::auth::claude::list_accounts()?
                .iter()
                .any(|account| account.label == label)
            {
                anyhow::bail!("No Claude account with label '{}' found", label);
            }
            ActiveProvider::Claude
        } else {
            if !crate::auth::codex::list_accounts()?
                .iter()
                .any(|account| account.label == label)
            {
                anyhow::bail!("No OpenAI account with label '{}' found", label);
            }
            ActiveProvider::OpenAI
        };
        self.reconcile_auth_if_provider_missing(target);
        let runtime = match target {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => unreachable!(),
        }
        .ok_or_else(|| anyhow!("{} runtime is not configured", Self::provider_label(target)))?;
        let replacement = runtime.fork();
        replacement.set_oauth_account(Some(label))?;
        replacement.set_credential_mode(CredentialMode::OAuth)?;
        replacement.set_model(&model_name_for_provider(target, model.trim()))?;
        let slot = match target {
            ActiveProvider::Claude => &self.anthropic,
            ActiveProvider::OpenAI => &self.openai,
            _ => unreachable!(),
        };
        *slot.write().unwrap_or_else(|p| p.into_inner()) = Some(replacement);
        self.set_active_provider(target);
        self.invalidate_routes_memo();
        Ok(())
    }

    pub(super) fn selected_account_for_provider(&self, target: ActiveProvider) -> Option<String> {
        let runtime = match target {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => None,
        }?;
        (runtime.credential_mode() == CredentialMode::OAuth)
            .then(|| runtime.oauth_account_label())
            .flatten()
    }
}
