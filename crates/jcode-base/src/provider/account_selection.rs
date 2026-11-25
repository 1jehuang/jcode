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
        let replacement = runtime.fork();
        if let Some(pin) = runtime.oauth_account_pin() {
            replacement.set_oauth_account_pin(&pin)?;
        } else {
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
            replacement.set_oauth_account(Some(&label))?;
        }
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
        self.set_model_on_oauth_account(target, Some(label), model, None)
    }

    pub(super) fn set_model_on_oauth_account(
        &self,
        target: ActiveProvider,
        account: Option<&str>,
        model: &str,
        fingerprint: Option<&str>,
    ) -> Result<()> {
        self.reconcile_auth_if_provider_missing(target);
        let runtime = match target {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => unreachable!(),
        }
        .ok_or_else(|| anyhow!("{} runtime is not configured", Self::provider_label(target)))?;
        let replacement = runtime.fork();
        replacement.set_oauth_account(account)?;
        replacement.set_credential_mode(CredentialMode::OAuth)?;
        let model = model_name_for_provider(target, model.trim());
        if let Some(fingerprint) = fingerprint {
            replacement.set_route_selection_verified(
                &RouteSelection {
                    model: model.into_owned(),
                    runtime_key: RuntimeKey::ClaudeOAuth,
                    api_method: "claude-oauth".into(),
                    provider_label: "Claude".into(),
                    detail: String::new(),
                },
                fingerprint,
            )?;
        } else {
            replacement.set_model(&model)?;
        }
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

impl MultiProvider {
    pub(super) fn select_saved_oauth_account(&self, label: Option<&str>) -> Result<()> {
        let active = self.active_provider();
        if let Some(label) = label {
            let method = match active {
                ActiveProvider::Claude => "claude-oauth",
                ActiveProvider::OpenAI => "openai-oauth",
                _ => anyhow::bail!("This provider does not support saved OAuth accounts"),
            };
            return self.set_model_on_saved_account(method, label, &self.model());
        }
        let runtime = match active {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => None,
        };
        if let Some(runtime) = runtime {
            runtime.set_oauth_account(None)?;
        }
        self.invalidate_routes_memo();
        Ok(())
    }
}

// Retain identity evidence without reconstructing a pin from its display label.
pub(super) fn fork_saved_account_runtime(
    runtime: Option<Arc<dyn Provider>>,
    key: &str,
) -> Option<Arc<dyn Provider>> {
    let runtime = runtime?;
    if runtime.oauth_account_pin().is_some() {
        Some(runtime.fork())
    } else {
        external::instantiate_expected_external_provider(key)
    }
}

impl MultiProvider {
    pub(super) fn fork_model_switch_request(
        &self,
        active: ActiveProvider,
        current_model: &str,
    ) -> String {
        if let Some(label) = self.selected_account_for_provider(active) {
            let method = if active == ActiveProvider::Claude {
                "claude-oauth"
            } else {
                "openai-oauth"
            };
            return format!("{method}@{label}:{current_model}");
        }
        let prefix = match active {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    // OAuth/ApiKey emit their canonical model prefix; Auto keeps
                    // the bare provider key (route without pinning a credential).
                    anthropic
                        .credential_mode()
                        .auth_route(jcode_provider_core::DualAuthProvider::Anthropic)
                        .map(|route| route.model_prefix())
                        .unwrap_or("claude")
                } else {
                    "claude"
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    openai
                        .credential_mode()
                        .auth_route(jcode_provider_core::DualAuthProvider::OpenAI)
                        .map(|route| route.model_prefix())
                        .unwrap_or("openai")
                } else {
                    "openai"
                }
            }
            ActiveProvider::Copilot => "copilot",
            ActiveProvider::Antigravity => "antigravity",
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Cursor => "cursor",
            ActiveProvider::Bedrock => "bedrock",
            ActiveProvider::OpenRouter => {
                if let Some(openrouter) = self.active_openrouter_execution_provider()
                    && let Some((_provider, api_method, _detail)) =
                        openrouter.direct_openai_compatible_route_parts()
                    && let Some(profile_id) = api_method
                        .strip_prefix("openai-compatible:")
                        .map(str::trim)
                        .filter(|profile_id| !profile_id.is_empty())
                {
                    return format!("{profile_id}:{current_model}");
                }
                if let Some(openrouter) = self.openrouter_provider()
                    && let Some(provider_pin) = openrouter.explicit_provider_pin_for_current_model()
                {
                    return format!("openrouter:{current_model}@{provider_pin}");
                }
                "openrouter"
            }
        };
        format!("{prefix}:{current_model}")
    }
}
