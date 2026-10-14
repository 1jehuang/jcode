use super::*;
use anyhow::Result;

fn account_override(provider: ActiveProvider) -> Option<String> {
    match provider {
        ActiveProvider::Claude => crate::auth::claude::get_active_account_override(),
        ActiveProvider::OpenAI => crate::auth::codex::get_active_account_override(),
        _ => None,
    }
}

/// An alternate login is provisional until its request is accepted. Restoring
/// it must also work when the caller drops the completion future during startup.
struct AccountFailoverRestore<'a> {
    router: &'a MultiProvider,
    provider: ActiveProvider,
    original_override: Option<String>,
    selected_override: Option<String>,
    runtime: Option<Arc<dyn Provider>>,
    committed: bool,
}

impl<'a> AccountFailoverRestore<'a> {
    fn new(router: &'a MultiProvider, provider: ActiveProvider) -> Self {
        let slot = match provider {
            ActiveProvider::Claude => &router.anthropic,
            ActiveProvider::OpenAI => &router.openai,
            _ => unreachable!("account failover requires a multi-account provider"),
        };
        Self {
            router,
            provider,
            original_override: account_override(provider),
            selected_override: None,
            runtime: slot.read().unwrap_or_else(|p| p.into_inner()).clone(),
            committed: false,
        }
    }

    fn select(&mut self, label: &str) {
        self.selected_override = Some(label.to_owned());
        set_account_override_for_provider(self.provider, self.selected_override.clone());
    }
}

impl Drop for AccountFailoverRestore<'_> {
    fn drop(&mut self) {
        if self.committed
            || self.selected_override.is_none()
            || account_override(self.provider) != self.selected_override
        {
            return;
        }
        // Do not undo a different session's runtime replacement or account
        // selection. No await is permitted between the ownership check and reset.
        let slot = match self.provider {
            ActiveProvider::Claude => &self.router.anthropic,
            ActiveProvider::OpenAI => &self.router.openai,
            _ => return,
        };
        let mut current = slot.write().unwrap_or_else(|p| p.into_inner());
        if !matches!((&*current, &self.runtime), (Some(current), Some(original)) if Arc::ptr_eq(current, original))
        {
            return;
        }
        set_account_override_for_provider(self.provider, self.original_override.clone());
        if let Some(runtime) = current.as_ref() {
            // Anthropic forks have an empty credential cache. OpenAI forks share
            // credentials, so reload synchronously against the restored override
            // and discard their persistent connection state as well.
            let replacement = runtime.fork();
            if self.provider == ActiveProvider::OpenAI {
                replacement.reload_credentials();
            }
            *current = Some(replacement);
        }
    }
}

impl MultiProvider {
    pub(super) async fn try_same_provider_account_failover(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        initial_reason: &str,
        notes: &mut Vec<String>,
    ) -> Result<Option<EventStream>> {
        if self.selected_account_for_provider(provider).is_some()
            || !same_provider_account_failover_enabled()
            || !self.provider_uses_oauth_accounts(provider)
        {
            return Ok(None);
        }

        let original_label = active_account_label_for_provider(provider);
        let Some(original_label) = original_label else {
            return Ok(None);
        };

        let alternatives = same_provider_account_candidates(provider);
        if alternatives.is_empty() {
            return Ok(None);
        }

        let provider_key = Self::provider_key(provider);
        let provider_label = Self::provider_label(provider);
        let mut restore = AccountFailoverRestore::new(self, provider);

        for alternative_label in &alternatives {
            crate::logging::info(&format!(
                "Same-provider failover{}: retrying {} using account '{}'",
                mode.log_suffix(),
                provider_label,
                alternative_label
            ));

            restore.select(alternative_label);
            if provider == ActiveProvider::OpenAI {
                clear_all_model_unavailability_for_account();
            }
            self.invalidate_provider_credentials_for_account_switch(provider)
                .await;

            let attempt = match mode {
                CompletionMode::Unified { system } => {
                    self.complete_on_provider(provider, messages, tools, system, None)
                        .await
                }
                CompletionMode::Split {
                    system_static,
                    system_dynamic,
                } => {
                    self.complete_split_on_provider(
                        provider,
                        messages,
                        tools,
                        system_static,
                        system_dynamic,
                        None,
                    )
                    .await
                }
            };
            let attempt = self.await_account_stream_start(provider, attempt).await;

            // Auto can fall back to an API key while loading an alternate's
            // credentials. That is not a successful OAuth account switch, and
            // an API-key error must not poison or retry more OAuth logins.
            if !self.provider_uses_oauth_accounts(provider) {
                drop(restore);
                return attempt.map(Some);
            }

            match attempt {
                Ok(stream) => {
                    restore.committed = true;
                    clear_provider_unavailable_for_account(provider_key);
                    self.record_provider_activity(provider);
                    self.startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(format!(
                        "⚡ Auto-switched {} account: {} → {}. To turn this off, set `[provider].same_provider_account_failover = false` in `~/.jcode/config.toml` or export `JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER=false`.",
                        provider_label, original_label, alternative_label
                    ));
                    return Ok(Some(stream));
                }
                Err(err) => {
                    let summary = Self::summarize_error(&err);
                    let display_summary = maybe_annotate_limit_summary(provider, summary.clone());
                    let decision = Self::classify_failover_error(&err);
                    crate::logging::info(&format!(
                        "Same-provider account {} failed{}: {} (failover={} decision={})",
                        alternative_label,
                        mode.log_suffix(),
                        display_summary,
                        decision.should_failover(),
                        decision.as_str()
                    ));
                    notes.push(format!(
                        "{} account {}: {}",
                        provider_label, alternative_label, display_summary
                    ));
                    if decision.should_mark_provider_unavailable() {
                        record_provider_unavailable_for_account(provider_key, &summary);
                    }
                    if !decision.should_failover() {
                        return Err(err);
                    }
                }
            }
        }

        drop(restore);
        if provider == ActiveProvider::OpenAI {
            clear_all_model_unavailability_for_account();
        }

        crate::logging::info(&format!(
            "Same-provider failover{} exhausted all alternate {} accounts after: {}",
            mode.log_suffix(),
            provider_label,
            initial_reason
        ));

        Ok(None)
    }
}
