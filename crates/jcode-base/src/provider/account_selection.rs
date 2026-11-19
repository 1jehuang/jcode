use super::*;

impl MultiProvider {
    /// Auto prefers saved subscriptions, but provider-level Auto can silently
    /// fall back to an API key when refresh fails. Bind the subscription before
    /// sending anything so only the pool/countdown router may change billing.
    pub(super) fn bind_auto_account_for_request(&self, target: ActiveProvider) -> Result<()> {
        if target == ActiveProvider::OpenAI
            && jcode_provider_core::is_chatgpt_web_model(&self.model())
        {
            return Ok(());
        }
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
        // login is known to be spent (local cooldown or cached quota, no network
        // IO), start on the next ready pool member instead of failing the first
        // request. Use the same exact-account usage check as the request guard:
        // usage reports can establish exhaustion before a request records 429.
        let label = match models::provider_unavailability_detail_for_account_label(
            Self::provider_key(target),
            Some(&label),
        )
        .or_else(|| {
            account_failover::multi_account_provider_kind(target)
                .filter(|kind| crate::usage::account_known_exhausted(*kind, &label))
                .map(|_| "OAuth usage exhausted".to_string())
        }) {
            Some(reason)
                if self
                    .account_failover
                    .enabled()
                    .unwrap_or_else(account_failover::same_provider_account_failover_config) =>
            {
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
            if let Some(instance) = self.claude_cli_instance(Some(label)) {
                return self.select_claude_via_cli(&instance, model.trim());
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
        let origin = self.approved_account_switch_origin(target);
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
        self.record_approved_account_switch(origin, label);
        self.invalidate_routes_memo();
        Ok(())
    }

    pub(super) fn saved_account_binding_exists(
        &self,
        target: ActiveProvider,
        label: &str,
    ) -> Result<bool> {
        match target {
            ActiveProvider::OpenAI => {
                if let Some(pin) = self.account_pin(AccountProviderKind::OpenAi) {
                    return Ok(crate::auth::codex::resolve_pin(&pin).as_deref() == Some(label));
                }
                Ok(crate::auth::codex::list_accounts()?
                    .iter()
                    .any(|account| account.label == label))
            }
            ActiveProvider::Claude => {
                if let Some(pin) = self.account_pin(AccountProviderKind::Claude) {
                    return Ok(crate::auth::claude::resolve_pin(&pin).as_deref() == Some(label));
                }
                Ok(crate::auth::claude::list_accounts()?
                    .iter()
                    .any(|account| account.label == label))
            }
            _ => Ok(true),
        }
    }

    /// Claude Code instance serving a native Claude selection: the given saved
    /// account, else the native runtime's pinned or active account. `None`
    /// when that account uses jcode's native API or the CLI is unavailable.
    pub(super) fn claude_cli_instance(&self, label: Option<&str>) -> Option<String> {
        self.claude_code_provider()?;
        let label = match label {
            Some(label) => label.to_string(),
            None => self
                .anthropic_provider()?
                .account_pin(AccountProviderKind::Claude)
                .and_then(|pin| crate::auth::claude::resolve_pin(&pin))
                .or_else(crate::auth::claude::active_account_label)?,
        };
        crate::auth::claude_code::instance_for_account(&label)
    }

    /// A native Claude model selection whose account runs through the CLI
    /// (the default for saved accounts); an API-key selection stays native.
    pub(super) fn claude_selection_via_cli(
        &self,
        mode: Option<CredentialMode>,
        model: &str,
    ) -> Option<Result<()>> {
        let mode = mode.or_else(|| self.anthropic_provider().map(|p| p.credential_mode()));
        if mode == Some(CredentialMode::ApiKey) {
            return None;
        }
        let instance = self.claude_cli_instance(None)?;
        Some(self.select_claude_via_cli(&instance, model))
    }

    /// Pinning a CLI-mode Claude account in a native Claude session moves the
    /// session to that account's Claude Code instance.
    pub(super) fn claude_pin_via_cli(
        &self,
        kind: AccountProviderKind,
        pin: &Option<AccountPin>,
    ) -> Option<Result<()>> {
        if kind != AccountProviderKind::Claude || self.active_provider() != ActiveProvider::Claude {
            return None;
        }
        let label = crate::auth::claude::resolve_pin(pin.as_ref()?)?;
        let instance = self.claude_cli_instance(Some(&label))?;
        Some((|| {
            if let Some(provider) = self.account_runtime(kind) {
                provider.set_account_pin(kind, pin.clone())?;
            }
            self.select_claude_via_cli(&instance, &self.model())
        })())
    }

    /// Serve a saved Claude account through the Claude Code CLI.
    pub(super) fn select_claude_via_cli(&self, instance: &str, model: &str) -> Result<()> {
        let pin = crate::auth::claude_code::pin_for_label(instance)?;
        self.set_account_pin(AccountProviderKind::ClaudeCode, Some(pin))?;
        self.set_model_on_provider(ActiveProvider::ClaudeCode, model)
    }

    pub(super) fn selected_account_for_provider(&self, target: ActiveProvider) -> Option<String> {
        let runtime = match target {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => None,
        }?;
        if target == ActiveProvider::OpenAI
            && jcode_provider_core::is_chatgpt_web_model(&runtime.model())
        {
            return None;
        }
        if runtime.credential_mode() != CredentialMode::OAuth {
            return None;
        }
        let kind = match target {
            ActiveProvider::Claude => AccountProviderKind::Claude,
            ActiveProvider::OpenAI => AccountProviderKind::OpenAi,
            _ => return None,
        };
        // The label is presentation, not account identity. Another process can
        // rename this login while this runtime and its session stay alive.
        // Keep an unresolved label so the request gate still fails closed for
        // a genuinely removed account rather than selecting the default.
        runtime.account_pin(kind).map_or_else(
            || runtime.oauth_account_label(),
            |pin| {
                let label = match kind {
                    AccountProviderKind::Claude => crate::auth::claude::resolve_pin(&pin),
                    AccountProviderKind::OpenAi => crate::auth::codex::resolve_pin(&pin),
                    _ => None,
                };
                Some(label.unwrap_or(pin.label))
            },
        )
    }
}
