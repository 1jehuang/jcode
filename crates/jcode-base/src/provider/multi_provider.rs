use super::*;
use anyhow::Result;
use futures::{
    Stream,
    task::{Context, Poll},
};
use std::pin::Pin;

struct LeasedEventStream {
    inner: EventStream,
    lease: Option<crate::auth::provider_pool::AccountLease>,
}

impl Stream for LeasedEventStream {
    type Item = anyhow::Result<jcode_message_types::StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let item = self.inner.as_mut().poll_next(context);
        if matches!(item, Poll::Ready(None)) {
            self.lease = None;
        }
        item
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
        request_lease: Option<crate::auth::provider_pool::AccountRequestLease>,
    ) -> Result<Option<EventStream>> {
        if !same_provider_account_failover_enabled() {
            return Ok(None);
        }

        let original_label = active_account_label_for_provider(provider);
        let Some(original_label) = original_label else {
            return Ok(None);
        };

        let model = self.model();
        let alternatives =
            account_failover::same_provider_account_candidates(provider, Some(&model));
        if alternatives.is_empty() {
            return Ok(None);
        }

        let provider_key = Self::provider_key(provider);
        let provider_label = Self::provider_label(provider);
        let request_lease = match request_lease {
            Some(lease) => lease,
            None => crate::auth::provider_pool::acquire_account_request_lease(provider_key)
                .await
                .expect("account provider failover must have a request lease"),
        };

        for alternative_label in &alternatives {
            let lease = crate::auth::provider_pool::try_acquire_account_lease(
                provider_key,
                alternative_label,
                std::time::Duration::from_secs(120),
            );
            if lease.is_none() {
                crate::logging::info(&format!(
                    "Same-provider failover{}: account '{}' is already leased",
                    mode.log_suffix(),
                    alternative_label
                ));
                continue;
            }
            crate::logging::info(&format!(
                "Same-provider failover{}: retrying {} using account '{}'",
                mode.log_suffix(),
                provider_label,
                alternative_label
            ));

            let request_scoped_account = matches!(
                provider,
                ActiveProvider::Antigravity | ActiveProvider::Cursor
            );
            if !request_scoped_account {
                set_account_override_for_provider(provider, Some(alternative_label.clone()));
            }
            clear_provider_unavailable_for_account(provider_key);
            if provider == ActiveProvider::OpenAI {
                clear_all_model_unavailability_for_account();
            }
            self.invalidate_provider_credentials_for_account_switch(provider)
                .await;

            let attempt = match mode {
                CompletionMode::Unified { system } => {
                    self.complete_on_provider_with_guard(
                        provider,
                        messages,
                        tools,
                        system,
                        None,
                        Some(request_lease.clone()),
                        Some(alternative_label),
                    )
                    .await
                }
                CompletionMode::Split {
                    system_static,
                    system_dynamic,
                } => {
                    self.complete_split_on_provider_with_guard(
                        provider,
                        messages,
                        tools,
                        system_static,
                        system_dynamic,
                        None,
                        Some(request_lease.clone()),
                        Some(alternative_label),
                    )
                    .await
                }
            };

            match attempt {
                Ok(stream) => {
                    if matches!(
                        provider,
                        ActiveProvider::Antigravity | ActiveProvider::Cursor
                    ) {
                        crate::auth::provider_pool::clear_account_cooldown(
                            Self::provider_key(provider),
                            alternative_label,
                        );
                    }
                    self.startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(format!(
                        "⚡ Auto-switched {} account: {} → {}. To turn this off, set `[provider].same_provider_account_failover = false` in `~/.jcode/config.toml` or export `JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER=false`.",
                        provider_label, original_label, alternative_label
                    ));
                    return Ok(Some(Box::pin(LeasedEventStream {
                        inner: stream,
                        lease,
                    })));
                }
                Err(err) => {
                    drop(lease);
                    let summary =
                        maybe_annotate_limit_summary(provider, Self::summarize_error(&err));
                    let decision = Self::classify_failover_error(&err);
                    crate::logging::info(&format!(
                        "Same-provider account {} failed{}: {} (failover={} decision={})",
                        alternative_label,
                        mode.log_suffix(),
                        summary,
                        decision.should_failover(),
                        decision.as_str()
                    ));
                    notes.push(format!(
                        "{} account {}: {}",
                        provider_label, alternative_label, summary
                    ));
                    if matches!(
                        provider,
                        ActiveProvider::Antigravity | ActiveProvider::Cursor
                    ) {
                        let default_cooldown = if summary.to_ascii_lowercase().contains("429")
                            || summary.to_ascii_lowercase().contains("rate limit")
                            || summary.to_ascii_lowercase().contains("quota")
                        {
                            std::time::Duration::from_secs(300)
                        } else {
                            std::time::Duration::from_secs(30)
                        };
                        let cooldown =
                            crate::auth::provider_pool::cooldown_for_error(&err, default_cooldown);
                        crate::auth::provider_pool::mark_account_cooldown(
                            Self::provider_key(provider),
                            alternative_label,
                            cooldown,
                        );
                    }
                    if decision.should_mark_provider_unavailable() {
                        record_provider_unavailable_for_account(provider_key, &summary);
                    }
                }
            }
        }

        if !matches!(
            provider,
            ActiveProvider::Antigravity | ActiveProvider::Cursor
        ) {
            set_account_override_for_provider(provider, Some(original_label));
            self.invalidate_provider_credentials_for_account_switch(provider)
                .await;
            if provider == ActiveProvider::OpenAI {
                clear_all_model_unavailability_for_account();
            }
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
