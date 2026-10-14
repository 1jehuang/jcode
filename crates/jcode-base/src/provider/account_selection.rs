use super::*;

impl MultiProvider {
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
